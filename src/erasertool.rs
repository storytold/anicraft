//! The Eraser (E) and the Eyedropper (I), ADR 0034: view state over `draw.erase`, `draw.faucet`
//! and `paint.pick`. An eraser drag collects samples like the Brush, shows the stage colour
//! where it goes and sends one command when the pointer lifts, so a stroke is one undo step.

use crate::app::{App, Drag, Tool};
use crate::brushtool::{BrushOptions, draw_sweep, draw_tip, points, shape_key};
use crate::pathtools::Xf;
use crate::theme::to_color32;
use anicraft_engine::cmds_erase::EraseMode;
use anicraft_engine::doc::ElementKind;
use anicraft_engine::geom::Vec2;
use anicraft_engine::geom::brush::TipShape;
use egui::Pos2;
use serde_json::json;

/// The Eraser's options (view state: the tool's, not the document's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EraserOptions {
    /// Diameter (or side), stage units.
    pub size: f32,
    pub shape: TipShape,
    pub mode: EraseMode,
    /// The Faucet: a click deletes a whole fill or line.
    pub faucet: bool,
    /// Use the Brush tool's tip (size, shape, angle and roundness) instead of the eraser's own.
    pub match_brush: bool,
}

impl Default for EraserOptions {
    fn default() -> Self {
        EraserOptions { size: 16.0, shape: TipShape::Round, mode: EraseMode::Normal, faucet: false, match_brush: false }
    }
}

impl EraserOptions {
    /// The effective tip as the Brush draws it (for `draw.erase` and the shared previews).
    pub(crate) fn tip(&self, brush: &BrushOptions) -> BrushOptions {
        if self.match_brush {
            BrushOptions { size: brush.size, shape: brush.shape, angle: brush.angle, roundness: brush.roundness, pressure_size: false, ..BrushOptions::default() }
        } else {
            BrushOptions { size: self.size, shape: self.shape, pressure_size: false, ..BrushOptions::default() }
        }
    }
}

/// Starts an eraser stroke at stage point `p` (the eraser ignores pen pressure).
pub fn start(app: &mut App, p: Vec2) -> Drag {
    app.pen_pressure = None;
    app.brush_samples = vec![(p, None)];
    Drag::Eraser
}

/// Sends the stroke: one command, one undo step.
pub fn commit(app: &mut App, xf: Xf) {
    let samples = std::mem::take(&mut app.brush_samples);
    if samples.is_empty() {
        return;
    }
    let tip = app.eraser.tip(&app.brush);
    let params = json!({"points": points(&samples), "size": tip.size, "shape": shape_key(tip.shape), "angle": tip.angle, "roundness": tip.roundness, "mode": app.eraser.mode.key(), "unit": xf.unit()});
    let undo = app.undo_key();
    // What the stroke passed over and kept (ADR 0093): symbols and drawing objects, which open
    // in place with a double-click so their own lines and fills can be erased.
    let kept = |v: &serde_json::Value| (v["kept"]["symbols"].as_u64().unwrap_or(0), v["kept"]["objects"].as_u64().unwrap_or(0));
    match app.run("draw.erase", params) {
        Some(v) if v["erased"] == true => match kept(&v) {
            (0, 0) => app.status_ok(tr!("Erased. {undo} to undo.", undo = undo)),
            _ => app.status_ok(tr!("Erased the lines and fills. {undo} to undo. Symbols and drawing objects are kept: double-click one with the Selection tool to erase inside it.", undo = undo)),
        },
        Some(v) if kept(&v) != (0, 0) => {
            let k = kept(&v);
            app.status_ok(match k {
                (_, 0) => tr!("Nothing erased: on the stage the Eraser keeps symbols. Double-click one with the Selection tool to erase inside it."),
                (0, _) => tr!("Nothing erased: the Eraser keeps drawing objects. Double-click one with the Selection tool to erase inside it."),
                _ => tr!("Nothing erased: the Eraser keeps symbols and drawing objects. Double-click one with the Selection tool to erase inside it."),
            });
        }
        Some(v) => {
            // The engine's reason is English from below the shell.
            let why = v["reason"].as_str().map_or(std::borrow::Cow::Borrowed(tr!("nothing to erase there")), crate::i18n::t_dyn);
            app.status_ok(tr!("Nothing erased: {why}. The eraser works on lines and fills drawn with Object Drawing off; drawing objects and symbols are kept.", why = why));
        }
        None => {}
    }
}

/// The Faucet: deletes the whole fill or line under the click.
pub fn faucet(app: &mut App, p: Vec2, zoom: f32) {
    if let Some(v) = app.run("draw.faucet", json!({"x": p.x, "y": p.y, "slop": 4.0 / zoom.max(1e-3)})) {
        let undo = app.undo_key();
        app.status_ok(if v["kind"] == "line" { tr!("Deleted the line. {undo} to undo.", undo = undo) } else { tr!("Deleted the fill. {undo} to undo.", undo = undo) });
    }
}

/// Double-clicking the Eraser tool: erase everything on the stage at this frame (asks first).
pub fn erase_everything(app: &mut App) {
    // The engine asks only when there is something to erase.
    if app.s.confirmation("draw.erase_all", &json!({})).is_none() {
        app.status_ok(tr!("Nothing to erase: the visible, unlocked layers are empty at this frame."));
        return;
    }
    app.request("draw.erase_all", json!({}));
}

/// The stroke as it goes (in the stage colour, as if erased) and the tip under the pointer.
pub fn draw_overlay(app: &App, painter: &egui::Painter, xf: Xf, hover: Option<Pos2>) {
    if app.tool != Tool::Eraser {
        return;
    }
    let tip = app.eraser.tip(&app.brush);
    if app.drag == Drag::Eraser {
        let bg = to_color32(anicraft_engine::doc::Rgba { a: 1.0, ..app.s.doc.stage.background });
        draw_sweep(painter, xf, &tip, &app.brush_samples, bg);
    }
    if let Some(h) = hover
        && !app.eraser.faucet
    {
        draw_tip(painter, xf, &tip, xf.to_stage(h));
    }
}

/// The Eyedropper's click. It picks the fill or line under the point into the tool settings
/// (Shift: into the other swatch only). As in Animate, it then becomes the Paint Bucket (a
/// fill) or the Ink Bottle (a line), and with shapes selected it gives them the picked style.
pub fn eyedropper(app: &mut App, p: Vec2, shift: bool, zoom: f32) {
    let Some(v) = app.run("paint.pick", json!({"x": p.x, "y": p.y, "other": shift, "slop": 3.0 / zoom.max(1e-3)})) else { return };
    let color = v["color"].as_str().unwrap_or("").to_string();
    let into_fill = v["into"] == "fill";
    if shift {
        app.status_ok(if into_fill { tr!("Picked {color} into the fill colour.", color = color) } else { tr!("Picked {color} into the stroke colour.", color = color) });
        return;
    }
    // One whole sentence per kind of paint, so each translates as a sentence.
    let paint = v["paint"].as_str();
    let width = v["width"].as_f64().map_or(String::new(), |w| format!("{w:.1}"));
    // The selected shapes on every layer (ADR 0044).
    let shapes = app.s.selected_elements().iter().filter(|(_, e)| matches!(e.kind, ElementKind::Shape(_))).count();
    if shapes > 0 {
        if let Some(r) = app.run("paint.apply_style", json!({"fill": into_fill, "stroke": !into_fill})) {
            let n = r["changed"].as_u64().unwrap_or(0);
            let undo = app.undo_key();
            app.status_ok(match paint {
                Some("linear") => trn!("Applied the picked linear gradient to {n} shape. {undo} to undo.", "Applied the picked linear gradient to {n} shapes. {undo} to undo.", n, undo = undo),
                Some("radial") => trn!("Applied the picked radial gradient to {n} shape. {undo} to undo.", "Applied the picked radial gradient to {n} shapes. {undo} to undo.", n, undo = undo),
                Some("bitmap") => trn!("Applied the picked bitmap fill to {n} shape. {undo} to undo.", "Applied the picked bitmap fill to {n} shapes. {undo} to undo.", n, undo = undo),
                _ if into_fill => trn!("Applied the picked fill {color} to {n} shape. {undo} to undo.", "Applied the picked fill {color} to {n} shapes. {undo} to undo.", n, color = color, undo = undo),
                _ => trn!("Applied the picked line {color}, {width} px to {n} shape. {undo} to undo.", "Applied the picked line {color}, {width} px to {n} shapes. {undo} to undo.", n, color = color, width = width, undo = undo),
            });
        }
    } else {
        let picked = match paint {
            Some("linear") => tr!("Picked the linear gradient.").to_string(),
            Some("radial") => tr!("Picked the radial gradient.").to_string(),
            Some("bitmap") => tr!("Picked the bitmap fill.").to_string(),
            _ if into_fill => tr!("Picked the fill {color}.", color = color),
            _ => tr!("Picked the line {color}, {width} px.", color = color, width = width),
        };
        let next = if into_fill { tr!("Click a fill or an enclosed area to pour it.") } else { tr!("Click a shape to give it this line.") };
        app.status_ok(format!("{picked} {next}"));
    }
    app.tool = if into_fill { Tool::PaintBucket } else { Tool::InkBottle };
}

/// The colour picker's eyedropper on the stage (ADR 0109, #288): while a picker popup is open,
/// the pointer over the stage is an eyedropper. A chip beside it shows the colour drawn under
/// it (`paint.sample`: fills, gradients, bitmaps, lines and effects, as the CPU renderer draws
/// them), and a click hands that colour to the picker, which takes it and closes. Escape closes
/// the picker without picking, like any popup. `at` is the pointer on screen and in the space
/// being edited; `zoom` is screen points per unit there.
pub fn picker_dropper(app: &mut App, ui: &egui::Ui, resp: &egui::Response, picker: egui::Id, at: Option<(Pos2, Vec2)>, zoom: f32) {
    let Some((screen, p)) = at.filter(|_| resp.hovered()) else { return };
    let ctx = ui.ctx();
    crate::colour::over_stage(ctx);
    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    // One screen pixel there; sampled again only when the pointer, the view or the document
    // changes.
    let scale = (zoom * ctx.pixels_per_point()).clamp(0.01, 64.0);
    let key = (app.s.state, p.x.to_bits(), p.y.to_bits(), scale.to_bits());
    let cache = egui::Id::unique("anicraft.stage.picker_dropper");
    type Sampled = ((anicraft_engine::StateId, u32, u32, u32), [u8; 3]);
    let known: Option<Sampled> = ctx.data(|d| d.get_temp(cache));
    let rgb = match known.filter(|(k, _)| *k == key) {
        Some((_, rgb)) => Some(rgb),
        None => {
            let sampled = app.s.execute("paint.sample", &json!({"x": p.x, "y": p.y, "scale": scale})).ok().and_then(|v| {
                let byte = |k: &str| v[k].as_u64().and_then(|b| u8::try_from(b).ok());
                Some([byte("r")?, byte("g")?, byte("b")?])
            });
            if let Some(rgb) = sampled {
                ctx.data_mut(|d| d.insert_temp::<Sampled>(cache, (key, rgb)));
            }
            sampled
        }
    };
    let Some(rgb) = rgb else { return };
    // The chip: the colour that a click takes, below and right of the pointer.
    let chip = egui::Rect::from_min_size(screen + egui::vec2(14.0, 14.0), egui::vec2(20.0, 20.0));
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, cache));
    let colour = anicraft_engine::doc::Rgba::new(f32::from(rgb[0]) / 255.0, f32::from(rgb[1]) / 255.0, f32::from(rgb[2]) / 255.0, 1.0);
    painter.rect_filled(chip.expand(1.0), crate::theme::T.rc(), crate::theme::T.text);
    painter.rect_filled(chip, crate::theme::T.rc(), to_color32(colour));
    painter.rect_stroke(chip, crate::theme::T.rc(), egui::Stroke::new(1.0, crate::theme::T.panel), egui::StrokeKind::Inside);
    if ui.input(|i| i.pointer.primary_clicked()) {
        crate::colour::pick_from_stage(ctx, picker, rgb);
        app.status_ok(tr!("Picked {colour} from the stage.", colour = crate::colour::to_hex([rgb[0], rgb[1], rgb[2], 255], false)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a vertical eraser stroke down the middle of a raw filled rectangle and returns the
    /// horizontal gap cut between the two remaining halves.
    fn erased_gap(configure: impl FnOnce(&mut App)) -> f32 {
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, anicraft_engine::Session::default(), Some(std::path::PathBuf::from("unused.png")));
        app.run("layer.new", json!({"name": "Ink"}));
        app.run("draw.object_drawing", json!({"on": false}));
        app.run("draw.rect", json!({"x": 100, "y": 100, "w": 300, "h": 200, "fill": "#336699", "stroke": null}));
        app.tool = Tool::Eraser;
        configure(&mut app);
        let xf = Xf { origin: Pos2::ZERO, zoom: 1.0 };
        start(&mut app, Vec2::new(250.0, 50.0));
        app.brush_samples.push((Vec2::new(250.0, 350.0), None));
        commit(&mut app, xf);
        let mut boxes: Vec<_> = app
            .s
            .visible_elements()
            .iter()
            .filter_map(|e| match &e.kind {
                ElementKind::Shape(sh) if sh.fill.is_some() => Some(sh.path.bounds().transformed(&e.transform)),
                _ => None,
            })
            .collect();
        boxes.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
        assert_eq!(boxes.len(), 2, "vertical stroke across the rectangle splits it in two: {boxes:?}");
        boxes[1].min.x - boxes[0].max.x
    }

    #[test]
    fn matched_eraser_stroke_and_preview_follow_brush_tip() {
        let unmatched_gap = erased_gap(|app| {
            app.eraser.size = 10.0;
            app.brush.size = 60.0;
            app.paint_brush.size = 120.0;
            app.eraser.match_brush = false;
        });
        let matched_gap = erased_gap(|app| {
            app.eraser.size = 10.0;
            app.brush.size = 60.0;
            app.paint_brush.size = 120.0;
            app.eraser.match_brush = true;
        });
        assert!(matched_gap > unmatched_gap * 2.0, "matched eraser should follow the larger Brush size: matched={matched_gap}, unmatched={unmatched_gap}");

        // Angle and roundness of the Brush carry over too when matched.
        let flat_wide = erased_gap(|app| {
            app.eraser.match_brush = true;
            app.brush.size = 60.0;
            app.brush.shape = TipShape::Square;
            app.brush.roundness = 20.0;
            app.brush.angle = 0.0;
        });
        let flat_narrow = erased_gap(|app| {
            app.eraser.match_brush = true;
            app.brush.size = 60.0;
            app.brush.shape = TipShape::Square;
            app.brush.roundness = 20.0;
            app.brush.angle = 90.0;
        });
        assert!(flat_wide > flat_narrow * 2.0, "matched eraser should follow the Brush's angle and roundness: wide={flat_wide}, narrow={flat_narrow}");

        // The on-stage tip preview follows the Brush's size when matched.
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, anicraft_engine::Session::default(), Some(std::path::PathBuf::from("unused.png")));
        app.tool = Tool::Eraser;
        app.eraser.size = 10.0;
        app.brush.size = 60.0;
        let xf = Xf { origin: Pos2::ZERO, zoom: 1.0 };
        let preview_width = |app: &App| {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| draw_overlay(app, ui.painter(), xf, Some(Pos2::new(250.0, 200.0))));
            out.textures_delta.clear();
            let mut bounds = egui::Rect::NOTHING;
            for cs in out.shapes {
                bounds = bounds.union(cs.shape.visual_bounding_rect());
            }
            bounds.width()
        };
        app.eraser.match_brush = false;
        let unmatched_preview = preview_width(&app);
        app.eraser.match_brush = true;
        let matched_preview = preview_width(&app);
        assert!(matched_preview > unmatched_preview * 2.0, "matched tip preview should be wider: matched={matched_preview}, unmatched={unmatched_preview}");
    }

    #[test]
    fn size_keys_follow_match_brush() {
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, anicraft_engine::Session::default(), Some(std::path::PathBuf::from("unused.png")));
        app.tool = Tool::Eraser;
        app.eraser.size = 15.0;
        app.brush.size = 40.0;
        app.paint_brush.size = 6.0;

        // Match off: [ and ] change only the eraser's own size.
        app.eraser.match_brush = false;
        crate::brushtool::resize(&mut app, true);
        let grown_eraser = app.eraser.size;
        assert!(grown_eraser > 15.0, "unmatched resize should grow eraser size");
        assert_eq!(app.brush.size, 40.0, "unmatched resize must leave Brush size alone");
        assert_eq!(app.paint_brush.size, 6.0, "Paint Brush is never involved");
        crate::brushtool::resize(&mut app, false);
        assert!(app.eraser.size < grown_eraser, "unmatched shrink should reduce eraser size");
        assert_eq!(app.brush.size, 40.0);

        // Match on: [ and ] with the Eraser change the Brush's size instead.
        let eraser_before = app.eraser.size;
        app.eraser.match_brush = true;
        crate::brushtool::resize(&mut app, true);
        let grown_brush = app.brush.size;
        assert!(grown_brush > 40.0, "matched resize should grow Brush size");
        assert_eq!(app.eraser.size, eraser_before, "matched resize leaves the eraser's own stored size untouched");
        assert_eq!(app.paint_brush.size, 6.0, "Paint Brush is never involved");
        crate::brushtool::resize(&mut app, false);
        assert!(app.brush.size < grown_brush, "matched shrink should reduce Brush size");
        assert_eq!(app.eraser.size, eraser_before);

        // Switching to Brush with match off never changes the eraser.
        app.eraser.match_brush = false;
        app.tool = Tool::Brush;
        let brush_before = app.brush.size;
        crate::brushtool::resize(&mut app, true);
        assert!(app.brush.size > brush_before);
        assert_eq!(app.eraser.size, eraser_before);
    }
}

