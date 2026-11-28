//! Modify › Shape dialogs: Expand Fill…, Soften Fill Edges… (ADR 0109) and Optimize Curves….
//! OK runs one `modify.expand_fill`, `modify.soften_fill_edges` or `modify.optimize`, one undo
//! step. What was entered last is offered again (view state), so a run of shapes gets the same
//! settings.

use crate::a11y::{Named, SliderLook, slider_parts};
use crate::app::App;
use crate::theme::{FORM_LABEL_DIALOG, Type, form_row, text, with_type};
use anicraft_engine::cmds_shape::{DEFAULT_STEPS, DEFAULT_STRENGTH, MAX_DISTANCE};
use anicraft_engine::geom::outline::MAX_SOFTEN_STEPS;
use egui::Key;
use serde_json::json;

/// Which dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Expand,
    Soften,
    Optimize,
}

impl Kind {
    fn command(self) -> &'static str {
        match self {
            Kind::Expand => "modify.expand_fill",
            Kind::Soften => "modify.soften_fill_edges",
            Kind::Optimize => "modify.optimize",
        }
    }
}

/// What Expand Fill and Soften Fill Edges hold, and what the next one starts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    /// Stage pixels.
    pub distance: f32,
    pub steps: u32,
    pub inset: bool,
}

impl Default for Values {
    /// Animate's: 4 px, 4 steps, Expand.
    fn default() -> Self {
        Values { distance: 4.0, steps: DEFAULT_STEPS, inset: false }
    }
}

/// What Optimize Curves holds, and what the next one starts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OptimizeValues {
    /// Simplification strength, 0..=100.
    pub strength: f32,
    pub repeat: bool,
}

impl Default for OptimizeValues {
    fn default() -> Self {
        OptimizeValues { strength: DEFAULT_STRENGTH, repeat: false }
    }
}

/// The dialog while it is open.
pub struct ShapeDialog {
    pub kind: Kind,
    pub values: Values,
    pub optimize: OptimizeValues,
    /// The first field takes the keyboard on the first frame.
    focus: bool,
}

/// What each dialog was last given.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Last {
    pub expand: Values,
    pub soften: Values,
    pub optimize: OptimizeValues,
}

/// Opens the dialog for the selection. With nothing selected the engine says so instead.
pub fn open(app: &mut App, kind: Kind) {
    if app.s.selection.is_empty() {
        let probe = if kind == Kind::Optimize { json!({}) } else { json!({"distance": 1}) };
        app.request(kind.command(), probe);
        return;
    }
    app.playing = false;
    let values = match kind {
        Kind::Expand => app.shape_last.expand,
        Kind::Soften => app.shape_last.soften,
        Kind::Optimize => Values::default(),
    };
    app.shape_dialog = Some(ShapeDialog { kind, values, optimize: app.shape_last.optimize, focus: true });
}

/// The command params Expand Fill or Soften Fill Edges runs.
pub fn params(kind: Kind, v: &Values) -> serde_json::Value {
    let mut p = json!({"distance": v.distance.clamp(0.05, MAX_DISTANCE), "direction": if v.inset { "inset" } else { "expand" }});
    if kind == Kind::Soften {
        p["steps"] = json!(v.steps.clamp(1, MAX_SOFTEN_STEPS));
    }
    p
}

/// The command params Optimize Curves runs.
pub fn optimize_params(v: &OptimizeValues) -> serde_json::Value {
    json!({"strength": v.strength.clamp(0.0, 100.0), "repeat": v.repeat})
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(d) = &mut app.shape_dialog else { return };
    let (mut submit, mut cancel) = (false, false);
    let (title, id) = match d.kind {
        Kind::Expand => (tr!("Expand Fill"), "expand_fill"),
        Kind::Soften => (tr!("Soften Fill Edges"), "soften_fill_edges"),
        Kind::Optimize => (tr!("Optimize Curves"), "optimize_curves"),
    };
    crate::windows::new(ctx, title).id(egui::Id::unique(id)).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
        ui.set_min_width(340.0);
        if d.kind == Kind::Optimize {
            form_row(ui, FORM_LABEL_DIALOG, tr!("Strength"), |ui| {
                let (bar, num) = with_type(ui, Type::Data, |ui| {
                    slider_parts(ui, &mut d.optimize.strength, 0.0..=100.0, tr!("Strength"), SliderLook { suffix: " %", max_decimals: Some(0), step: Some(1.0), logarithmic: false })
                });
                let r = bar
                    .union(num.clone())
                    .on_hover_text(tr!("Higher values drop more points and allow curves to move slightly further from the original"));
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                if num.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            });
            form_row(ui, FORM_LABEL_DIALOG, "", |ui| {
                ui.checkbox(&mut d.optimize.repeat, tr!("Multiple passes"))
                    .on_hover_text(tr!("Keep simplifying until the point count stops dropping"));
            });
            ui.label(text(Type::Caption, tr!("Removes extra points from selected lines and fill outlines while keeping their shape.")));
        } else {
            form_row(ui, FORM_LABEL_DIALOG, tr!("Distance"), |ui| {
                let tip = match d.kind {
                    Kind::Expand => tr!("How far the fill's edge moves, in stage pixels"),
                    Kind::Soften => tr!("How wide the soft edge is, in stage pixels"),
                    Kind::Optimize => "",
                };
                let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.values.distance).range(0.05..=MAX_DISTANCE).suffix(" px").speed(0.25).max_decimals(2))).named(tr!("Distance")).on_hover_text(tip);
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            });
            if d.kind == Kind::Soften {
                form_row(ui, FORM_LABEL_DIALOG, tr!("Steps"), |ui| {
                    let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut d.values.steps).range(1..=MAX_SOFTEN_STEPS).speed(0.1)))
                        .named(tr!("Number of steps"))
                        .on_hover_text(tr!("How many bands the soft edge is made of: more are smoother, and more shapes to draw"));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        submit = true;
                    }
                });
            }
            form_row(ui, FORM_LABEL_DIALOG, tr!("Direction"), |ui| {
                ui.vertical(|ui| {
                    let (out, inward) = match d.kind {
                        Kind::Expand => (tr!("The fill grows by the distance; its corners round"), tr!("The fill shrinks by the distance; its corners stay sharp")),
                        Kind::Soften => (tr!("The soft edge is added round the fill"), tr!("The soft edge is taken from the fill, which keeps its size")),
                        Kind::Optimize => ("", ""),
                    };
                    ui.radio_value(&mut d.values.inset, false, tr!("Expand")).on_hover_text(out);
                    ui.radio_value(&mut d.values.inset, true, tr!("Inset")).on_hover_text(inward);
                });
            });
            if d.kind == Kind::Soften {
                ui.label(text(Type::Caption, tr!("Each step is a band of the fill's colour, fainter the farther out. Works best on a fill without an outline.")));
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tr!("OK")).clicked() {
                submit = true;
            }
            if ui.button(tr!("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                cancel = true;
            }
        });
    });
    if submit {
        accept(app);
    } else if cancel {
        app.shape_dialog = None;
    }
}

/// OK: runs the command with the dialog's values and remembers them for the next time.
fn accept(app: &mut App) {
    let Some(d) = app.shape_dialog.take() else { return };
    match d.kind {
        Kind::Expand => {
            app.shape_last.expand = d.values;
            app.request(d.kind.command(), params(d.kind, &d.values));
        }
        Kind::Soften => {
            app.shape_last.soften = d.values;
            app.request(d.kind.command(), params(d.kind, &d.values));
        }
        Kind::Optimize => {
            app.shape_last.optimize = d.optimize;
            app.request(d.kind.command(), optimize_params(&d.optimize));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::{Document, ElementKind};

    fn app(ctx: &egui::Context) -> App {
        let mut app = App::with_context(ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")));
        app.run("draw.rect", json!({"x": 100, "y": 100, "w": 200, "h": 100, "fill": "#3366cc", "stroke": null, "object": true}));
        app
    }

    fn shapes(app: &App) -> usize {
        app.s.visible_elements().iter().filter(|e| matches!(e.kind, ElementKind::Shape(_))).count()
    }

    #[test]
    fn the_dialogs_run_their_commands_and_remember_what_was_entered() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        open(&mut app, Kind::Expand);
        let d = app.shape_dialog.as_mut().expect("the dialog opens on a selection");
        assert_eq!(d.values, Values::default(), "Animate's defaults the first time");
        d.values = Values { distance: 12.0, steps: 9, inset: true };
        accept(&mut app);
        assert!(app.shape_dialog.is_none());
        let b = app.s.selection_bounds();
        assert!((b.width() - 176.0).abs() < 0.01 && (b.height() - 76.0).abs() < 0.01, "inset by 12: {b:?}");
        assert_eq!(app.s.undo_label(), Some("Expand Fill…"));
        // The next Expand Fill offers the same; Soften Fill Edges keeps its own.
        open(&mut app, Kind::Expand);
        assert_eq!(app.shape_dialog.as_ref().map(|d| d.values), Some(Values { distance: 12.0, steps: 9, inset: true }));
        app.shape_dialog = None;
        open(&mut app, Kind::Soften);
        let d = app.shape_dialog.as_mut().expect("the dialog opens");
        assert_eq!(d.values, Values::default());
        d.values.steps = 3;
        d.values.distance = 9.0;
        accept(&mut app);
        assert_eq!(shapes(&app), 4, "the fill and three bands");
        assert_eq!(app.shape_last.soften.steps, 3);
        app.run("edit.undo", json!({}));
        assert_eq!(shapes(&app), 1, "one undo step");
    }

    #[test]
    fn the_dialogs_draw_without_changing_anything_and_need_a_selection() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        for kind in [Kind::Expand, Kind::Soften] {
            open(&mut app, kind);
            let before = app.s.doc.clone();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(&mut app, ui.ctx()));
            out.textures_delta.clear();
            assert!(app.shape_dialog.is_some());
            assert_eq!(app.s.doc, before);
            app.shape_dialog = None;
        }
        // Nothing selected: no dialog; the engine says why.
        app.run("edit.deselect", json!({}));
        open(&mut app, Kind::Soften);
        assert!(app.shape_dialog.is_none());
        // Values out of range are brought into it.
        assert_eq!(params(Kind::Soften, &Values { distance: 0.0, steps: 900, inset: false }), json!({"distance": 0.05f32, "direction": "expand", "steps": 50}));
        assert_eq!(params(Kind::Expand, &Values { distance: 5000.0, steps: 2, inset: true }), json!({"distance": 1000.0f32, "direction": "inset"}));
    }

    #[test]
    fn optimize_dialog_runs_remembers_and_passes_a11y() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::with_context(&ctx, Session::new(Document::default()), Some(std::path::PathBuf::from("unused.png")));
        // Circle with 60 segments.
        let mut p = anicraft_engine::geom::Path::new();
        for i in 0..60 {
            let a = i as f32 / 60.0 * std::f32::consts::TAU;
            let pt = anicraft_engine::geom::Vec2::new(200.0 + 60.0 * a.cos(), 150.0 + 60.0 * a.sin());
            if i == 0 {
                p.move_to(pt);
            } else {
                p.line_to(pt);
            }
        }
        p.close();
        app.run("draw.path", json!({"d": p.to_svg(), "fill": "#3366cc", "stroke": null, "object": true}));
        open(&mut app, Kind::Optimize);
        let before = app.s.doc.clone();
        let mut tree = None;
        for _ in 0..4 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(&mut app, ui.ctx()));
            out.textures_delta.clear();
            // The last frame's tree: the window's contents arrive a frame after the window.
            if let Some(update) = out.platform_output.accesskit_update {
                tree = Some(update);
            }
        }
        assert_eq!(app.s.doc, before, "drawing the dialog changes nothing");
        let tree = tree.expect("AccessKit update arrived for Optimize Curves dialog");
        let labels: Vec<&str> = tree.nodes.iter().filter_map(|(_, n)| n.label()).collect();
        assert!(labels.contains(&"Strength"), "slider named: {labels:?}");
        assert!(labels.contains(&"Multiple passes"), "checkbox named: {labels:?}");
        assert!(labels.contains(&"OK") && labels.contains(&"Cancel"), "buttons named: {labels:?}");
        let d = app.shape_dialog.as_mut().expect("dialog is open");
        assert_eq!(d.optimize, OptimizeValues::default());
        d.optimize = OptimizeValues { strength: 75.0, repeat: true };
        accept(&mut app);
        assert!(app.shape_dialog.is_none());
        assert_eq!(app.s.undo_label(), Some("Optimize Curves…"));
        assert_eq!(app.shape_last.optimize, OptimizeValues { strength: 75.0, repeat: true });
        assert_eq!(optimize_params(&OptimizeValues { strength: 150.0, repeat: false }), json!({"strength": 100.0f32, "repeat": false}));
    }

    #[test]
    fn optimize_dialog_submits_on_enter_in_numeric_strength_field() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = app(&ctx);
        open(&mut app, Kind::Optimize);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let frame_input = || egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        let mut tree = None;
        for _ in 0..3 {
            let mut out = ctx.run_ui(frame_input(), |ui| window(&mut app, ui.ctx()));
            out.textures_delta.clear();
            if let Some(update) = out.platform_output.accesskit_update {
                tree = Some(update);
            }
        }
        let tree = tree.expect("AccessKit update arrived");
        // Find the numeric Strength DragValue (named "Strength", not the Slider bar).
        let num_rect = tree
            .nodes
            .iter()
            .filter_map(|(_, n)| (n.label() == Some("Strength") && n.role() != egui::accesskit::Role::Slider).then(|| n.bounds()).flatten())
            .next()
            .expect("numeric Strength field in AccessKit tree");
        let pos = egui::pos2(((num_rect.x0 + num_rect.x1) * 0.5) as f32, ((num_rect.y0 + num_rect.y1) * 0.5) as f32);
        // Click the numeric field to enter text editing, type a new value, then press Enter.
        for pressed in [true, false] {
            let mut inp = frame_input();
            inp.events.push(egui::Event::PointerMoved(pos));
            inp.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            let mut out = ctx.run_ui(inp, |ui| window(&mut app, ui.ctx()));
            out.textures_delta.clear();
        }
        // Let focus settle on the numeric field and type a new value before pressing Enter.
        let mut type_inp = frame_input();
        type_inp.events.push(egui::Event::Text("75".into()));
        let mut out = ctx.run_ui(type_inp, |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.shape_dialog.is_some(), "dialog stays open while typing");
        let mut enter = frame_input();
        enter.events.push(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        let mut out = ctx.run_ui(enter, |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.shape_dialog.is_none(), "pressing Enter in the numeric Strength field submits the dialog");
        assert_eq!(app.shape_last.optimize.strength, 75.0);
    }
}
