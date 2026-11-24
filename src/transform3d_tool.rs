//! The 3D Rotation tool (`Tool::Transform3D`, `G`, `P-044`): interactive stage gimbal and
//! Properties controls for 3D Postcard X/Y/Z rotation, Z depth, perspective FOV and vanishing
//! point.

use crate::a11y::{self, Named};
use crate::app::{App, Drag, Tool};
use crate::pathtools::Xf;
use crate::theme::{T, Type, text};
use anicraft_engine::doc::{ElementKind, Transform3D};
use anicraft_engine::geom::Vec2;
use anicraft_engine::geom::perspective3d::{DEFAULT_FOV_DEG, MAX_FOV_DEG, MIN_FOV_DEG};
use egui::{Pos2, Stroke, pos2};
use serde_json::json;

/// Degrees of rotation per stage-screen drag point.
const DEG_PER_PT: f32 = 0.45;
/// Pixels of Z translation per screen drag point when Alt is held.
const Z_PER_PT: f32 = 2.0;

/// Click with `Tool::Transform3D`: selects the element under the pointer.
pub fn click(app: &mut App, sp: Pos2, shift: bool) {
    app.bone_sel = None;
    app.run("element.pick", json!({"x": sp.x, "y": sp.y, "add": shift}));
}

/// Starts a `Tool::Transform3D` drag on the stage: selects under pointer if nothing is selected,
/// records starting 3D state, and begins one undo gesture.
pub fn drag_start(app: &mut App, press: Pos2, sp: Pos2, shift: bool, zoom: f32) -> Drag {
    if app.s.selection.is_empty() {
        if app.s.hit_test(Vec2::new(sp.x, sp.y), 3.0 / zoom.max(1e-3)).is_some() {
            app.run("element.pick", json!({"x": sp.x, "y": sp.y, "add": shift}));
        } else {
            return Drag::None;
        }
    }
    let Some((_, el)) = app.s.selected_elements().into_iter().find(|(_, e)| !matches!(e.kind, ElementKind::Camera)) else {
        return Drag::None;
    };
    let start_3d = el.transform_3d.unwrap_or_default().sanitized();
    app.s.begin_gesture(tr_noop!("3D Rotation"));
    Drag::Transform3D {
        press,
        rx: start_3d.rx_deg,
        ry: start_3d.ry_deg,
        rz: start_3d.rz_deg,
        tz: start_3d.tz,
    }
}

/// Updates `modify.set_3d_transform` live during a pointer drag:
/// - Default drag: horizontal = Yaw (`ry`), vertical = Pitch (`rx`)
/// - `Shift` held: horizontal/vertical = Roll (`rz`)
/// - `Alt` held: vertical = Depth (`z`)
#[allow(clippy::too_many_arguments)]
pub fn drag_to(
    app: &mut App,
    press: Pos2,
    base_rx: f32,
    base_ry: f32,
    base_rz: f32,
    base_tz: f32,
    cur_screen: Pos2,
    shift: bool,
    alt: bool,
) {
    let d = cur_screen - press;
    let params = if alt {
        json!({ "z": (base_tz + d.y * Z_PER_PT).clamp(-100_000.0, 100_000.0) })
    } else if shift {
        let mut rz = base_rz + d.x * DEG_PER_PT;
        rz = (rz / 5.0).round() * 5.0;
        json!({ "rz": rz })
    } else {
        json!({
            "rx": base_rx - d.y * DEG_PER_PT,
            "ry": base_ry + d.x * DEG_PER_PT,
        })
    };
    let _ = app.s.execute("modify.set_3d_transform", &params);
}

/// Draws the 3D gimbal ring and vanishing-point crosshair on the stage when `Tool::Transform3D` is in hand.
pub fn draw(app: &App, painter: &egui::Painter, xf: Xf) {
    if app.tool != Tool::Transform3D || app.s.selection.is_empty() {
        return;
    }
    let bounds = app.s.selection_bounds();
    if bounds.is_empty() {
        return;
    }
    let center = xf.to_screen(bounds.center());
    let radius = (bounds.width().hypot(bounds.height()) * 0.35 * xf.zoom).clamp(28.0, 96.0);
    painter.circle_stroke(center, radius, Stroke::new(1.5, T.selection));
    painter.line_segment(
        [pos2(center.x - radius, center.y), pos2(center.x + radius, center.y)],
        Stroke::new(1.0, T.accent),
    );
    painter.line_segment(
        [pos2(center.x, center.y - radius), pos2(center.x, center.y + radius)],
        Stroke::new(1.0, T.tween_motion),
    );
    painter.circle_filled(center, 3.5, T.selection);

    // Vanishing-point reticle.
    let vp_stage = app
        .s
        .selected_elements()
        .into_iter()
        .find_map(|(_, e)| e.transform_3d.and_then(|t| t.vanishing))
        .unwrap_or(Vec2::new(app.s.doc.stage.width as f32 * 0.5, app.s.doc.stage.height as f32 * 0.5));
    let vp = xf.to_screen(vp_stage);
    let arm = 6.0;
    let vp_stroke = Stroke::new(1.0, T.text_dim);
    painter.line_segment([pos2(vp.x - arm, vp.y), pos2(vp.x + arm, vp.y)], vp_stroke);
    painter.line_segment([pos2(vp.x, vp.y - arm), pos2(vp.x, vp.y + arm)], vp_stroke);
}

/// Numeric 3D Postcard controls inside the Properties panel (shown when `Tool::Transform3D` is active
/// or when a selected element carries a 3D transform).
pub fn properties_rows(
    app: &mut App,
    ui: &mut egui::Ui,
    current: Option<Transform3D>,
    mut row_fn: impl FnMut(&mut egui::Ui, &str, &mut dyn FnMut(&mut egui::Ui)),
    mut live_fn: impl FnMut(&mut App, &'static str, &'static str, serde_json::Value),
) {
    let t = current.unwrap_or_default().sanitized();
    let (mut rx, mut ry, mut rz) = (t.rx_deg, t.ry_deg, t.rz_deg);
    let mut tz = t.tz;
    let mut fov = t.fov_deg;

    row_fn(ui, tr!("3D Rotate"), &mut |ui| {
        let a = ui
            .add(egui::DragValue::new(&mut rx).prefix("X ").suffix("°").speed(0.5).range(-3600.0..=3600.0).max_decimals(1))
            .named(tr!("3D Rotation X"));
        let b = ui
            .add(egui::DragValue::new(&mut ry).prefix("Y ").suffix("°").speed(0.5).range(-3600.0..=3600.0).max_decimals(1))
            .named(tr!("3D Rotation Y"));
        let c = ui
            .add(egui::DragValue::new(&mut rz).prefix("Z ").suffix("°").speed(0.5).range(-3600.0..=3600.0).max_decimals(1))
            .named(tr!("3D Rotation Z"));
        if a.changed() || b.changed() || c.changed() {
            live_fn(app, tr_noop!("3D Rotation"), "modify.set_3d_transform", json!({"rx": rx, "ry": ry, "rz": rz}));
        }
    });

    row_fn(ui, tr!("3D Depth"), &mut |ui| {
        let z_drag = ui
            .add(egui::DragValue::new(&mut tz).prefix("Z ").suffix(" px").speed(1.0).range(-100_000.0..=100_000.0).max_decimals(1))
            .named(tr!("3D Position Z"));
        let fov_look = a11y::SliderLook { suffix: "°", max_decimals: Some(0), ..Default::default() };
        let fov_slider = a11y::slider(ui, &mut fov, MIN_FOV_DEG..=MAX_FOV_DEG, tr!("Perspective FOV"), fov_look);
        if z_drag.changed() || fov_slider.changed() {
            live_fn(app, tr_noop!("3D Depth"), "modify.set_3d_transform", json!({"z": tz, "fov": fov}));
        }
    });

    let stage_center = Vec2::new(app.s.doc.stage.width as f32 * 0.5, app.s.doc.stage.height as f32 * 0.5);
    let vp = t.vanishing.unwrap_or(stage_center);
    let (mut vx, mut vy) = (vp.x, vp.y);
    row_fn(ui, tr!("Vanishing"), &mut |ui| {
        let a = ui
            .add(egui::DragValue::new(&mut vx).prefix("X ").speed(1.0).max_decimals(1))
            .named(tr!("Vanishing point X"));
        let b = ui
            .add(egui::DragValue::new(&mut vy).prefix("Y ").speed(1.0).max_decimals(1))
            .named(tr!("Vanishing point Y"));
        if a.changed() || b.changed() {
            live_fn(app, tr_noop!("Vanishing Point"), "modify.set_3d_transform", json!({"vanishing": [vx, vy]}));
        }
        if ui
            .add_enabled(current.is_some() || (fov - DEFAULT_FOV_DEG).abs() > 1e-3, egui::Button::new(text(Type::Label, tr!("Reset"))))
            .named(tr!("Reset 3D transform"))
            .clicked()
        {
            app.run("modify.reset_3d_transform", json!({}));
        }
    });
}
