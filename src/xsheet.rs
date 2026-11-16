//! Vertical X-sheet (ADR 0100): the timeline stood on its end.
//!
//! Rows are frames and columns are layers, on the same playhead and the same keyframes as the
//! filmstrip below the stage. A cell's menu is the timeline's frame menu, plus placing a sound
//! from the Library. Nothing here is a second clock.

use crate::a11y;
use crate::app::App;
use crate::icons::{self, Icon};
use crate::menus;
use crate::theme::{T, TARGET, Type, font};
use crate::timeline::{self, FrameSel};
use anicraft_engine::doc::{Layer, LayerId, LayerKind, TweenKind};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, WidgetType, pos2, vec2};
use serde_json::json;
use std::sync::Arc;

/// Frame-number gutter. Wide enough for five digits, and a click target on its short side
/// because the row is `TARGET` tall.
const GUTTER: f32 = 52.0;
/// One layer. Wide enough for a key mark, a tween letter and a short sound name.
const COL_W: f32 = 88.0;
const HEADER: f32 = TARGET;
const ROW_H: f32 = TARGET;

/// Layers the sheet shows: the timeline's rows, skipping what a closed folder hides.
fn column_ids(app: &App) -> Vec<LayerId> {
    let tl = app.s.timeline();
    (0..tl.layers.len()).filter(|&i| !app.tl_collapsed.iter().any(|f| tl.is_inside(i, *f))).filter_map(|i| tl.layers.get(i).map(|l| l.id)).collect()
}

/// Arrow keys while the pointer is over the sheet: up and down move the playhead, left and
/// right the active layer. A text field or an open menu keeps the keys.
pub fn keys(app: &mut App, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() || egui::Popup::is_any_open(ctx) {
        return;
    }
    let now = ctx.cumulative_pass_nr();
    let Some((rect, pass)) = app.xsheet_hit else { return };
    if pass + 1 < now {
        return;
    }
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
    if !over {
        return;
    }
    let (mut frame_step, mut layer_step) = (0i64, 0i64);
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
            frame_step -= 1;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
            frame_step += 1;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
            layer_step -= 1;
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
            layer_step += 1;
        }
    });
    if frame_step != 0 {
        app.playing = false;
        app.timeline_focus = true;
        let last = i64::from(app.s.frame_count().saturating_sub(1));
        let to = (i64::from(app.s.frame) + frame_step).clamp(0, last.max(0));
        app.run("timeline.goto", json!({"frame": to}));
    }
    if layer_step != 0 {
        let cols = column_ids(app);
        let at = cols.iter().position(|id| *id == app.s.layer).unwrap_or(0);
        let n = cols.len().saturating_sub(1);
        let to = (at as i64 + layer_step).clamp(0, n as i64) as usize;
        if let Some(id) = cols.get(to).copied().filter(|id| *id != app.s.layer) {
            app.playing = false;
            app.timeline_focus = true;
            app.run("layer.select", json!({"layer": id.0}));
        }
    }
}

/// The panel: a pinned frame gutter and layer header, and a scrolling grid of cells.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let body = ui.available_rect_before_wrap();
    ui.allocate_rect(body, Sense::hover());
    let pass = ui.ctx().cumulative_pass_nr();
    app.xsheet_hit = Some((body, pass));

    let cols = column_ids(app);
    let layers = app.s.timeline().layers.clone();
    let names: Vec<(u64, String)> = app.s.doc.sounds.iter().map(|s| (s.id.0, s.name.clone())).collect();
    let frames = app.s.frame_count().max(1);
    let painter = ui.painter_at(body);
    painter.rect_filled(body, 0.0, T.panel);

    let grid_area = Rect::from_min_max(pos2(body.left() + GUTTER, body.top() + HEADER), body.right_bottom());
    let mut grid_ui = ui.new_child(egui::UiBuilder::new().max_rect(grid_area));
    let out = egui::ScrollArea::both().id_salt("xsheet_grid").auto_shrink([false, false]).show(&mut grid_ui, |ui| {
        let wide = (cols.len() as f32 * COL_W).max(COL_W);
        let tall = frames as f32 * ROW_H;
        let (content, resp) = ui.allocate_exact_size(vec2(wide, tall), a11y::pointer_only());
        a11y::control(ui, &resp, WidgetType::Button, tr!("X-Sheet"), None);
        let columns = paint_cells(ui, app, &layers, &cols, &names, frames, content);
        // Frame 0's screen y. A drop on a column uses it to name the frame.
        app.drop.xsheet = Some(crate::drop::XSheetGeo { columns, y0: content.top(), row_h: ROW_H });
        cells_pointer(app, ui, &resp, &layers, &cols, frames, content);
        resp.context_menu(|ui| {
            if app.s.active_layer().is_some_and(|l| l.kind == LayerKind::Folder) {
                layer_menu(app, ui);
            } else {
                cell_menu(app, ui);
            }
        });
        follow(ui, app, &cols, content);
        content
    });

    let offset = out.state.offset;
    let view = out.inner_rect;
    let content = out.inner;
    chrome(ui, app, &layers, &cols, frames, body, view, content, offset);
}

/// Brings the playhead's cell back into view when the playhead or the active layer moved,
/// and leaves a scroll the user made alone.
fn follow(ui: &egui::Ui, app: &mut App, cols: &[LayerId], content: Rect) {
    let here = (app.s.frame, app.s.layer.0);
    if app.xsheet_follow == Some(here) {
        return;
    }
    app.xsheet_follow = Some(here);
    let ci = cols.iter().position(|id| *id == app.s.layer).unwrap_or(0) as f32;
    let cell = Rect::from_min_size(pos2(content.left() + ci * COL_W, content.top() + app.s.frame as f32 * ROW_H), vec2(COL_W, ROW_H));
    if !ui.clip_rect().contains_rect(cell) {
        ui.scroll_to_rect(cell, None);
    }
}

fn paint_cells(ui: &egui::Ui, app: &App, layers: &[Arc<Layer>], cols: &[LayerId], names: &[(u64, String)], frames: u32, content: Rect) -> Vec<(Rect, LayerId)> {
    let p = ui.painter();
    let clip = ui.clip_rect();
    p.rect_filled(content.intersect(clip), 0.0, T.panel);
    let mut drop_cols = Vec::new();
    if cols.is_empty() || frames == 0 {
        return drop_cols;
    }
    let c0 = ((clip.left() - content.left()) / COL_W).floor().max(0.0) as usize;
    let c1 = (((clip.right() - content.left()) / COL_W).ceil().max(0.0) as usize).min(cols.len().saturating_sub(1));
    let f0 = ((clip.top() - content.top()) / ROW_H).floor().max(0.0) as u32;
    let f1 = (((clip.bottom() - content.top()) / ROW_H).ceil().max(0.0) as u32).min(frames.saturating_sub(1));
    if c0 > c1 || f0 > f1 {
        return drop_cols;
    }
    for ci in c0..=c1 {
        let Some(id) = cols.get(ci).copied() else { continue };
        let Some((i, layer)) = layers.iter().enumerate().find(|(_, l)| l.id == id) else { continue };
        let x = content.left() + ci as f32 * COL_W;
        let col = Rect::from_min_size(pos2(x, content.top()), vec2(COL_W, frames as f32 * ROW_H)).intersect(clip);
        let folder = layer.kind == LayerKind::Folder;
        if !folder && col.width() > 1.0 && col.height() > 1.0 {
            drop_cols.push((col, id));
        }
        for f in f0..=f1 {
            let cell = Rect::from_min_size(pos2(x, content.top() + f as f32 * ROW_H), vec2(COL_W, ROW_H));
            paint_cell(p, app, layers, i, layer, f, cell, names, folder);
        }
        p.vline(x + COL_W - 0.5, clip.y_range().intersection(content.y_range()), Stroke::new(1.0, T.separator));
    }
    drop_cols
}

#[allow(clippy::too_many_arguments)]
fn paint_cell(p: &egui::Painter, app: &App, layers: &[Arc<Layer>], i: usize, layer: &Layer, f: u32, cell: Rect, names: &[(u64, String)], folder: bool) {
    let play = f == app.s.frame;
    if folder {
        p.rect_filled(cell, 0.0, if play { T.row_selected } else { T.panel_alt });
        return;
    }
    let k = layer.keyframe_at(f);
    let (fill, ink, letter, key, empty, sound) = match k {
        None => (if play { T.row_selected } else { T.panel }, if play { T.on_selected } else { T.text_faint }, None, false, true, None),
        Some(k) => {
            let ki = layer.keyframe_index_at(f).unwrap_or(0);
            let next = layer.keyframes.get(ki + 1).is_some_and(|n| n.start == k.end());
            let motion = k.motion.is_some();
            let shape = k.tween.as_ref().is_some_and(|t| t.kind == TweenKind::Shape);
            let tweened = k.tween.is_some() && next && !motion;
            let empty = k.elements.is_empty();
            let fill = if motion {
                T.tween_motion
            } else if tweened && shape {
                T.tween_shape
            } else if tweened {
                T.tween
            } else if empty {
                T.span_empty
            } else {
                T.span
            };
            let letter = if motion {
                Some('M')
            } else if tweened && shape {
                Some('S')
            } else if tweened {
                Some('C')
            } else {
                None
            };
            let ink = if play {
                T.on_selected
            } else if empty && !tweened && !motion {
                T.text
            } else {
                T.ink
            };
            let sound = (k.start == f).then_some(k.sound.as_ref()).flatten().and_then(|fs| names.iter().find(|(id, _)| *id == fs.sound.0).map(|(_, n)| n.clone()));
            let held = k.sound.is_some() && k.start != f;
            (if play { T.row_selected } else { fill }, ink, letter, f == k.start, empty, sound.or_else(|| held.then(String::new)))
        }
    };
    p.rect_filled(cell, 0.0, fill);
    if play
        && let Some(k) = k
    {
        let bar = span_color(k, layer);
        p.rect_filled(Rect::from_min_size(cell.left_top(), vec2(4.0, cell.height())), 0.0, bar);
    }
    if (f + 1).is_multiple_of(5) {
        p.hline(cell.x_range(), cell.bottom() - 0.5, Stroke::new(1.0, T.border));
    } else {
        p.hline(cell.x_range(), cell.bottom() - 0.5, Stroke::new(1.0, T.separator));
    }
    if key {
        let c = pos2(cell.left() + 14.0, cell.center().y);
        if empty && letter.is_none() {
            p.circle_stroke(c, 4.0, Stroke::new(1.5, ink));
        } else {
            p.circle_filled(c, 4.0, ink);
        }
    } else if letter.is_some() || sound.as_ref().is_some_and(|s| s.is_empty()) {
        // A held frame of a tween or a sound: a short tick, as a tracker leaves the row blank
        // once the note has started.
        let c = pos2(cell.left() + 14.0, cell.center().y);
        p.vline(c.x, (c.y - 5.0)..=(c.y + 5.0), Stroke::new(1.5, ink));
    }
    if let Some(ch) = letter {
        p.text(pos2(cell.left() + 24.0, cell.center().y), Align2::LEFT_CENTER, ch.to_string(), font(Type::Data), ink);
    }
    if let Some(name) = sound.as_ref().filter(|s| !s.is_empty()) {
        let icon = Rect::from_center_size(pos2(cell.right() - 12.0, cell.center().y), vec2(12.0, 12.0));
        icons::paint(p, icon, Icon::Sound, if play { T.on_selected } else { T.waveform });
        let galley = elide(p, name, font(Type::Caption), ink, (cell.width() - 40.0).max(4.0));
        p.galley(pos2(cell.left() + 36.0, cell.center().y - galley.size().y * 0.5), galley, ink);
    }
    if app.frame_sel.is_some_and(|s| s.contains(layers, i, f)) {
        p.rect_stroke(cell.shrink(1.0), 0.0, Stroke::new(2.0, T.selection), StrokeKind::Inside);
    }
}

/// The span colour the horizontal timeline would use, kept as a bar on the white playhead row
/// so a tween still reads there.
fn span_color(k: &anicraft_engine::doc::Keyframe, layer: &Layer) -> Color32 {
    let ki = layer.keyframe_index_at(k.start).unwrap_or(0);
    let next = layer.keyframes.get(ki + 1).is_some_and(|n| n.start == k.end());
    if k.motion.is_some() {
        T.tween_motion
    } else if k.tween.is_some() && next && k.tween.as_ref().is_some_and(|t| t.kind == TweenKind::Shape) {
        T.tween_shape
    } else if k.tween.is_some() && next {
        T.tween
    } else if k.elements.is_empty() {
        T.span_empty
    } else {
        T.span
    }
}

fn cells_pointer(app: &mut App, ui: &egui::Ui, resp: &egui::Response, layers: &[Arc<Layer>], cols: &[LayerId], frames: u32, content: Rect) {
    let Some(p) = resp.interact_pointer_pos().or(resp.hover_pos()) else { return };
    let Some((ci, f)) = hit(p, content, cols.len(), frames) else { return };
    let Some(id) = cols.get(ci).copied() else { return };
    let go = resp.clicked() || resp.dragged() || resp.secondary_clicked() || resp.double_clicked();
    if !go {
        return;
    }
    let shift = ui.input(|i| i.modifiers.shift);
    let ctrl = ui.input(|i| i.modifiers.command);
    if resp.secondary_clicked() {
        let inside = app.frame_sel.is_some_and(|s| {
            let i = layers.iter().position(|l| l.id == id);
            i.is_some_and(|i| s.contains(layers, i, f))
        });
        if !inside {
            app.frame_sel = Some(FrameSel::cell(id, f));
        }
    } else if resp.dragged() && !resp.drag_started() {
        if let Some(sel) = app.frame_sel.as_mut() {
            sel.head = (id, f);
        } else {
            app.frame_sel = Some(FrameSel::cell(id, f));
        }
    } else if shift && resp.clicked() {
        match app.frame_sel.as_mut() {
            Some(sel) => sel.head = (id, f),
            None => app.frame_sel = Some(FrameSel::cell(id, f)),
        }
    } else if resp.clicked() || resp.drag_started() {
        let whole = layers.iter().find(|l| l.id == id).and_then(|l| timeline::whole_motion_span(l, f));
        app.frame_sel = Some(whole.filter(|_| !ctrl).unwrap_or_else(|| FrameSel::cell(id, f)));
    }
    app.playing = false;
    app.timeline_focus = true;
    if app.s.layer != id {
        app.run("layer.select", json!({"layer": id.0}));
    }
    app.run("timeline.goto", json!({"frame": f}));
    if resp.double_clicked() && layers.iter().find(|l| l.id == id).and_then(|l| l.keyframe_at(f)).is_some_and(|k| k.motion.is_some()) {
        app.show_panel(crate::dock::Panel::MotionEditor);
    }
}

fn hit(p: egui::Pos2, content: Rect, n_cols: usize, n_frames: u32) -> Option<(usize, u32)> {
    if n_cols == 0 || n_frames == 0 {
        return None;
    }
    let i = ((p.x - content.left()) / COL_W).floor();
    let f = ((p.y - content.top()) / ROW_H).floor();
    if i < 0.0 || f < 0.0 {
        return None;
    }
    let i = i as usize;
    let f = f as u32;
    (i < n_cols && f < n_frames).then_some((i, f))
}

/// Layer names across the top and frame numbers down the side. Both stay put while the grid
/// scrolls; `offset` is the grid's scroll.
#[allow(clippy::too_many_arguments)]
fn chrome(ui: &mut egui::Ui, app: &mut App, layers: &[Arc<Layer>], cols: &[LayerId], frames: u32, body: Rect, view: Rect, content: Rect, offset: egui::Vec2) {
    let header = Rect::from_min_max(pos2(view.left(), body.top()), pos2(view.right(), view.top().max(body.top() + HEADER)));
    let gutter = Rect::from_min_max(pos2(body.left(), view.top()), pos2(view.left().max(body.left() + GUTTER), view.bottom()));
    let corner = Rect::from_min_max(body.left_top(), pos2(gutter.right(), header.bottom()));
    let p = ui.painter();
    p.rect_filled(corner, 0.0, T.panel_alt);
    p.rect_filled(header, 0.0, T.panel_alt);
    p.rect_filled(gutter, 0.0, T.panel_alt);
    p.text(corner.center(), Align2::CENTER_CENTER, tr!("Frame"), font(Type::Caption), T.text_dim);
    p.line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, T.border));
    p.line_segment([gutter.right_top(), gutter.right_bottom()], Stroke::new(1.0, T.border));

    let hp = p.with_clip_rect(header);
    for (ci, id) in cols.iter().copied().enumerate() {
        let x = view.left() + ci as f32 * COL_W - offset.x;
        let cell = Rect::from_min_size(pos2(x, header.top()), vec2(COL_W, header.height()));
        if !header.intersects(cell) {
            continue;
        }
        let Some(layer) = layers.iter().find(|l| l.id == id) else { continue };
        let on = app.s.layer == id;
        hp.rect_filled(cell.intersect(header), 0.0, if on { T.row_selected } else { T.panel_alt });
        let ink = if on { T.on_selected } else if layer.locked || !layer.visible { T.text_faint } else { T.text };
        if layer.kind == LayerKind::Folder {
            icons::paint(&hp, Rect::from_center_size(pos2(cell.left() + 12.0, cell.center().y), vec2(12.0, 12.0)), Icon::Folder, ink);
        }
        let galley = elide(&hp, &layer.name, font(Type::Label), ink, cell.width() - if layer.kind == LayerKind::Folder { 28.0 } else { 10.0 });
        let left = if layer.kind == LayerKind::Folder { cell.left() + 22.0 } else { cell.left() + 6.0 };
        hp.galley(pos2(left, cell.center().y - galley.size().y * 0.5), galley, ink);
        hp.vline(cell.right() - 0.5, header.y_range(), Stroke::new(1.0, T.separator));
    }
    let gp = p.with_clip_rect(gutter);
    let f0 = ((gutter.top() - content.top()) / ROW_H).floor().max(0.0) as u32;
    let f1 = (((gutter.bottom() - content.top()) / ROW_H).ceil().max(0.0) as u32).min(frames.saturating_sub(1));
    let y0 = content.top();
    if f0 <= f1 {
        for f in f0..=f1 {
            let y = y0 + f as f32 * ROW_H;
            let row = Rect::from_min_max(pos2(gutter.left(), y), pos2(gutter.right(), y + ROW_H)).intersect(gutter);
            if row.height() < 1.0 {
                continue;
            }
            let play = f == app.s.frame;
            if play {
                gp.rect_filled(row, 0.0, T.row_selected);
                gp.rect_filled(Rect::from_min_size(row.left_top(), vec2(3.0, row.height())), 0.0, T.playhead);
            } else if (f + 1) % 5 == 0 {
                gp.hline(row.x_range(), row.bottom() - 0.5, Stroke::new(1.0, T.border));
            }
            gp.text(pos2(row.right() - 8.0, row.center().y), Align2::RIGHT_CENTER, (f + 1).to_string(), font(Type::Data), if play { T.on_selected } else { T.text });
        }
    }

    let heads = ui.interact(header, ui.id().with("xsheet_heads"), a11y::pointer_only());
    a11y::control(ui, &heads, WidgetType::Button, tr!("Layer names"), None);
    if let Some(p) = heads.interact_pointer_pos().filter(|_| heads.clicked() || heads.secondary_clicked()) {
        let i = ((p.x - view.left() + offset.x) / COL_W).floor();
        if i >= 0.0
            && let Some(id) = cols.get(i as usize).copied()
        {
            app.playing = false;
            app.run("layer.select", json!({"layer": id.0}));
        }
    }
    heads.context_menu(|ui| layer_menu(app, ui));

    let nums = ui.interact(gutter, ui.id().with("xsheet_frames"), a11y::pointer_only());
    a11y::control(ui, &nums, WidgetType::Button, tr!("Frame numbers"), None);
    if let Some(p) = nums.interact_pointer_pos().filter(|_| nums.clicked() || nums.secondary_clicked()) {
        let f = ((p.y - y0) / ROW_H).floor().max(0.0) as u32;
        let f = f.min(frames.saturating_sub(1));
        app.playing = false;
        app.timeline_focus = true;
        app.frame_sel = Some(FrameSel::cell(app.s.layer, f));
        app.run("timeline.goto", json!({"frame": f}));
    }
    nums.context_menu(|ui| cell_menu(app, ui));
}

fn elide(p: &egui::Painter, text: &str, font_id: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font_id, color);
    job.wrap = egui::text::TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    p.layout_job(job)
}

/// The frame menu, then the sounds in the Library. The cell is already selected.
fn cell_menu(app: &mut App, ui: &mut egui::Ui) {
    timeline::frame_menu(app, ui);
    ui.separator();
    let frame = app.s.frame;
    let Some((id, folder, sounding)) = app.s.active_layer().map(|l| (l.id.0, l.kind == LayerKind::Folder, l.keyframe_at(frame).is_some_and(|k| k.sound.is_some()))) else {
        return;
    };
    if folder {
        ui.add_enabled(false, egui::Button::new(tr!("Sounds go on a layer, not a folder")));
        return;
    }
    if sounding && ui.button(tr!("Remove Sound")).clicked() {
        app.run("sound.remove", json!({"layer": id, "frame": frame}));
        ui.close();
    }
    let sounds: Vec<(u64, String)> = app.s.doc.sounds.iter().map(|s| (s.id.0, s.name.clone())).collect();
    if sounds.is_empty() {
        ui.add_enabled(false, egui::Button::new(tr!("No sounds in the Library")));
        return;
    }
    ui.menu_button(tr!("Place Sound"), |ui| {
        for (sid, name) in &sounds {
            if ui.button(name).clicked() {
                app.run("sound.place", json!({"sound": sid, "layer": id, "frame": frame, "extend": true}));
                ui.close();
            }
        }
    });
}

fn layer_menu(app: &mut App, ui: &mut egui::Ui) {
    ui.set_min_width(200.0);
    for id in ["layer.toggle_visible", "layer.toggle_lock", "layer.toggle_outline"] {
        menus::cmd_item(app, ui, id);
    }
    let (sounds, muted) = app.s.active_layer().map(|l| (l.has_sounds(), l.muted)).unwrap_or((false, false));
    if sounds && menus::check_item(ui, tr!("Mute"), muted, true) {
        app.run("layer.toggle_mute", json!({}));
    }
    ui.separator();
    for id in ["layer.move_up", "layer.move_down", "layer.delete"] {
        menus::cmd_item(app, ui, id);
    }
}

