//! Colour swatches and the colour picker (#201, #246, #248), as Animate's swatch popup: a click
//! on a swatch opens the web-safe palette with the colours used lately below it. Along the top
//! are the colour and its hex, which follow the swatch under the pointer (#434, #435), and at
//! the right end one button that turns the palette into the mixer (hue, saturation and value)
//! and back (#437). When the colour has opacity there is an Alpha row (0 % clear, 100 % opaque).
//!
//! The 216 web-safe colours are the open standard cube (each channel 00, 33, 66, 99, CC or FF),
//! laid out as the six-by-six blocks artists know from Animate's default swatches (#432): red
//! steps from block to block, green across a block and blue down it, so every block runs from
//! its darkest corner at the top left to its lightest at the bottom right. A column of greys
//! and primaries sits on its left. The colours are the public standard's; only their order was
//! learned, by looking at the program.
//!
//! View state only: what a colour is used for is the caller's (a tool's swatch, or a command).

use crate::a11y::Named;
use crate::theme::{T, Type, text, to_color32, with_type};
use anicraft_engine::doc::Rgba;
use egui::{Color32, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, pos2, vec2};

/// One palette cell, in points.
const CELL: f32 = 14.0;
/// Columns of the web-safe palette, and its rows.
const COLS: usize = 18;
const ROWS: usize = 12;
/// Most colours kept in the recent row.
const RECENT: usize = COLS + 1;
/// Most rows of document swatches the picker shows (the Swatches panel has them all).
const DOC_ROWS: usize = 3;

/// The channel steps of the web-safe cube.
const STEPS: [u8; 6] = [0x00, 0x33, 0x66, 0x99, 0xcc, 0xff];

/// The web-safe colour at `row` (0..12), `col` (0..18): blocks of six by six, red by block,
/// green across the block, blue down it.
pub fn web_safe(row: usize, col: usize) -> Option<[u8; 3]> {
    if row >= ROWS || col >= COLS {
        return None;
    }
    let red = (row / 6) * 3 + col / 6;
    Some([*STEPS.get(red)?, *STEPS.get(col % 6)?, *STEPS.get(row % 6)?])
}

/// The column left of the palette: greys from black to white, then the primaries and
/// secondaries.
const SIDE: [[u8; 3]; ROWS] = [
    [0x00, 0x00, 0x00],
    [0x33, 0x33, 0x33],
    [0x66, 0x66, 0x66],
    [0x99, 0x99, 0x99],
    [0xcc, 0xcc, 0xcc],
    [0xff, 0xff, 0xff],
    [0xff, 0x00, 0x00],
    [0x00, 0xff, 0x00],
    [0x00, 0x00, 0xff],
    [0xff, 0xff, 0x00],
    [0x00, 0xff, 0xff],
    [0xff, 0x00, 0xff],
];

/// `#rrggbb`, or `#rrggbbaa` when `alpha` and it isn't opaque.
pub fn to_hex(c: [u8; 4], alpha: bool) -> String {
    if alpha && c[3] != 255 {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
    } else {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    }
}

/// Reads `rgb`, `rrggbb` or (with `alpha`) `rgba`, `rrggbbaa`, with or without one `#`, and
/// spaces around it. The alpha is opaque unless given.
pub fn parse_hex(s: &str, alpha: bool) -> Option<[u8; 4]> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| s.get(i..i + 1).and_then(|d| u8::from_str_radix(d, 16).ok());
    let pair = |i: usize| s.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok());
    match (s.len(), alpha) {
        (3, _) => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255]),
        (4, true) => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, digit(3)? * 17]),
        (6, _) => Some([pair(0)?, pair(2)?, pair(4)?, 255]),
        (8, true) => Some([pair(0)?, pair(2)?, pair(4)?, pair(6)?]),
        _ => None,
    }
}

fn c32(c: [u8; 4]) -> Color32 {
    to_color32(Rgba::new(f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, f32::from(c[3]) / 255.0))
}

/// What the picker shows first: the swatches (Animate's default) or the mixer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Swatches,
    Mixer,
}

/// The colours used lately, newest first, and the popup that added the newest (a drag in the
/// mixer or typing in the hex field changes that one entry instead of adding one per step).
#[derive(Clone, Default)]
struct Recent {
    colours: Vec<[u8; 3]>,
    from: Option<Id>,
}

fn recent_id() -> Id {
    Id::unique("anicraft.colour.recent")
}

fn remember(ctx: &egui::Context, popup: Id, c: [u8; 3]) {
    ctx.data_mut(|d| {
        let r: &mut Recent = d.get_temp_mut_or_default(recent_id());
        if r.from == Some(popup) && !r.colours.is_empty() {
            r.colours.remove(0);
        }
        r.colours.retain(|q| *q != c);
        r.colours.insert(0, c);
        r.colours.truncate(RECENT);
        r.from = Some(popup);
    });
}

/// The picker's eyedropper (ADR 0109, #288): while a picker popup is open, a click on the stage
/// takes the colour drawn there. The picker and the stage are apart in the interface, so they
/// meet here, by frame number: the picker says it is showing, the stage says the pointer is
/// over it and hands back what it sampled, and the picker takes that and closes.
#[derive(Clone, Copy, Default)]
struct Dropper {
    /// The picker that showed last, and the frame it showed in.
    open: Option<(Id, u64)>,
    /// The frame the pointer was last over the stage with a picker open.
    over_stage: Option<u64>,
    /// A colour sampled from the stage for picker `.0`, and the frame it was sampled in.
    picked: Option<(Id, [u8; 3], u64)>,
}

fn dropper_id() -> Id {
    Id::unique("anicraft.colour.dropper")
}

fn dropper(ctx: &egui::Context) -> Dropper {
    ctx.data(|d| d.get_temp(dropper_id())).unwrap_or_default()
}

/// Whether something recorded in frame `then` still counts: this frame or the one before (the
/// stage and the panels draw in either order).
fn fresh(ctx: &egui::Context, then: u64) -> bool {
    ctx.cumulative_frame_nr().saturating_sub(then) <= 1
}

/// The picker that takes a colour clicked on the stage: one showing in a popup now.
pub(crate) fn picking(ctx: &egui::Context) -> Option<Id> {
    dropper(ctx).open.filter(|(_, frame)| fresh(ctx, *frame)).map(|(id, _)| id)
}

/// The stage, each frame the pointer is over it while a picker is open: the popup then stays
/// open through a click there.
pub(crate) fn over_stage(ctx: &egui::Context) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Dropper>(dropper_id()).over_stage = Some(frame));
}

/// The stage hands picker `to` the colour clicked on it.
pub(crate) fn pick_from_stage(ctx: &egui::Context, to: Id, c: [u8; 3]) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Dropper>(dropper_id()).picked = Some((to, c, frame)));
    ctx.request_repaint();
}

/// The colour the stage handed picker `popup`, once (one never collected is dropped).
fn take_picked(ctx: &egui::Context, popup: Id) -> Option<[u8; 3]> {
    let (to, c, frame) = dropper(ctx).picked?;
    let stale = !fresh(ctx, frame);
    if stale || to == popup {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Dropper>(dropper_id()).picked = None);
    }
    (!stale && to == popup).then_some(c)
}

/// How a picker popup closes: on a click outside it, except a click on the stage, which picks
/// a colour first (the picker closes itself once it has it).
pub(crate) fn close_behavior(ctx: &egui::Context) -> egui::PopupCloseBehavior {
    if dropper(ctx).over_stage.is_some_and(|frame| fresh(ctx, frame)) { egui::PopupCloseBehavior::IgnoreClicks } else { egui::PopupCloseBehavior::CloseOnClickOutside }
}

/// A new popup opening adds its first change as a new recent colour.
pub(crate) fn new_session(ctx: &egui::Context) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Recent>(recent_id()).from = None);
}

/// A swatch button for an opaque colour: a click opens the picker. `changed()` when the colour
/// changed this frame. Name it with `Named::named`.
pub fn button(ui: &mut egui::Ui, c: &mut [u8; 3]) -> Response {
    let mut full = [c[0], c[1], c[2], 255];
    let r = button_inner(ui, &mut full, false);
    *c = [full[0], full[1], full[2]];
    r
}

/// A swatch button for a colour with alpha (unmultiplied).
pub fn button_alpha(ui: &mut egui::Ui, c: &mut [u8; 4]) -> Response {
    button_inner(ui, c, true)
}

fn button_inner(ui: &mut egui::Ui, c: &mut [u8; 4], alpha: bool) -> Response {
    let size = ui.spacing().interact_size;
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::new(egui::Role::ColorWell));
    let popup = swatch_popup_id(resp.id);
    let open = egui::Popup::is_id_open(ui.ctx(), popup);
    if ui.is_rect_visible(rect) {
        let v = if open { &ui.visuals().widgets.open } else { ui.style().interact(&resp) };
        let rect = rect.expand(v.expansion);
        egui::color_picker::show_color_at(ui.painter(), c32(*c), rect.shrink(1.0));
        ui.painter().rect_stroke(rect, T.rc(), Stroke::new(1.0, if open || resp.hovered() { T.selection } else { T.border }), StrokeKind::Inside);
    }
    if popup_for(ui, &resp, popup, c, alpha, None) {
        resp.mark_changed();
    }
    resp
}

pub(crate) fn swatch_popup_id(anchor: Id) -> Id {
    anchor.with("colour_popup")
}

/// Opens the picker under `anchor` when it is clicked, and shows it while open. True when the
/// colour changed. `none`, when given, offers No Colour (true: none); picking a colour turns it
/// off again.
pub fn popup_for(ui: &egui::Ui, anchor: &Response, popup: Id, c: &mut [u8; 4], alpha: bool, none: Option<&mut bool>) -> bool {
    if anchor.clicked() {
        new_session(ui.ctx());
    }
    let mut changed = false;
    let shown = egui::Popup::menu(anchor).id(popup).close_behavior(close_behavior(ui.ctx())).show(|ui| {
        name_popup(ui, tr!("Colour picker"));
        changed = picker(ui, popup, c, alpha, none, true);
    });
    match shown {
        Some(_) => {}
        // A tap that reached the stage and closed the popup in one frame still picks.
        None => {
            if let Some(p) = take_picked(ui.ctx(), popup) {
                *c = [p[0], p[1], p[2], c[3]];
                changed = true;
            }
        }
    }
    changed
}

/// Names a picker popup for screen readers: a dialog holding the picker's controls. Called
/// first thing inside the popup, on its own `Ui`: that is the node its controls hang under
/// (since egui 0.37 the popup's response is another node, with nothing in it).
pub(crate) fn name_popup(ui: &egui::Ui, name: &str) {
    // Where the popup was last frame: a container has no place of its own in the tree.
    let at = ui.ctx().memory(|m| m.area_rect(ui.layer_id().id));
    let scale = f64::from(ui.ctx().pixels_per_point());
    ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
        node.set_role(egui::accesskit::Role::Dialog);
        node.set_label(name);
        if let Some(r) = at {
            node.set_bounds(egui::accesskit::Rect { x0: f64::from(r.min.x) * scale, y0: f64::from(r.min.y) * scale, x1: f64::from(r.max.x) * scale, y1: f64::from(r.max.y) * scale });
        }
    });
}

/// The picker itself: swatches or mixer, then the colour, its hex and its alpha. With
/// `close_on_pick`, a click on a swatch is the choice and closes the popup (#570); a picker
/// that is one part of a larger popup (a gradient's colour stop) stays open instead.
pub(crate) fn picker(ui: &mut egui::Ui, popup: Id, c: &mut [u8; 4], alpha: bool, none: Option<&mut bool>, close_on_pick: bool) -> bool {
    let view_id = Id::unique("anicraft.colour.view");
    let mut view: View = ui.data(|d| d.get_temp(view_id)).unwrap_or_default();
    let mut changed = false;
    // The eyedropper: this picker is the one a click on the stage gives its colour to; the
    // alpha stays as set (what is sampled is the colour as drawn, which is opaque).
    let frame = ui.ctx().cumulative_frame_nr();
    ui.data_mut(|d| d.get_temp_mut_or_default::<Dropper>(dropper_id()).open = Some((popup, frame)));
    if let Some(p) = take_picked(ui.ctx(), popup) {
        *c = [p[0], p[1], p[2], c[3]];
        remember(ui.ctx(), popup, p);
        changed = true;
        egui::Popup::close_all(ui.ctx());
    }
    // Along the top, where Animate has them: the colour and its hex, which show the swatch
    // under the pointer while there is one (#434, #435); at the right end, No Colour and the one
    // button that turns the palette into the mixer and back (#437).
    let hover_id = popup.with("hovered");
    let hovered: Option<[u8; 3]> = if view == View::Swatches { ui.data(|d| d.get_temp(hover_id)) } else { None };
    let shown = hovered.map_or(*c, |h| [h[0], h[1], h[2], c[3]]);
    let width = CELL * (COLS + 1) as f32 + CELL * 0.5;
    ui.allocate_ui_with_layout(vec2(width, ui.spacing().interact_size.y), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(CELL * 2.5, ui.spacing().interact_size.y), Sense::hover());
        egui::color_picker::show_color_at(ui.painter(), c32(shown), r);
        ui.painter().rect_stroke(r, T.rc(), Stroke::new(1.0, T.border), StrokeKind::Inside);
        if let Some(h) = hex_field(ui, popup, c, shown, alpha) {
            *c = h;
            remember(ui.ctx(), popup, [h[0], h[1], h[2]]);
            changed = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if view_toggle(ui, view).clicked() {
                view = if view == View::Swatches { View::Mixer } else { View::Swatches };
                ui.data_mut(|d| d.insert_temp(view_id, view));
            }
            if let Some(none) = none
                && ui.selectable_label(*none, tr!("No colour")).on_hover_text(tr!("Draw without this colour")).clicked()
            {
                *none = !*none;
                changed = true;
            }
        });
    });
    ui.add_space(4.0);
    match view {
        View::Swatches => {
            let (picked, over) = swatches(ui, [c[0], c[1], c[2]]);
            if over != hovered {
                // The top row follows the pointer from the next frame.
                ui.data_mut(|d| match over {
                    Some(o) => {
                        d.insert_temp(hover_id, o);
                    }
                    None => d.remove::<[u8; 3]>(hover_id),
                });
                ui.ctx().request_repaint();
            }
            if let Some(p) = picked {
                *c = [p[0], p[1], p[2], c[3]];
                changed = true;
                if close_on_pick {
                    egui::Popup::close_all(ui.ctx());
                }
            }
        }
        View::Mixer => {
            let mut m = c32(*c);
            ui.spacing_mut().slider_width = (CELL * (COLS + 1) as f32).max(200.0);
            let a = if alpha { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
            if egui::color_picker::color_picker_color32(ui, &mut m, a) {
                let [r, g, b, al] = m.to_srgba_unmultiplied();
                *c = [r, g, b, if alpha { al } else { 255 }];
                remember(ui.ctx(), popup, [r, g, b]);
                changed = true;
            }
        }
    }
    ui.add_space(4.0);
    ui.label(text(Type::Caption, tr!("Or click anything on the stage to take its colour.")));
    // Its own row, with the word on it. A bare percent beside the hex does not read as opacity.
    if alpha {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let tip = tr!("Alpha: 0 % is clear, 100 % opaque");
            ui.label(tr!("Alpha")).on_hover_text(tip);
            let mut pct = f32::from(c[3]) / 255.0 * 100.0;
            ui.spacing_mut().slider_width = 148.0;
            // The theme's slider rail is the popup's own colour, so the bar would vanish into it.
            let name = tr!("Alpha");
            let bar = ui.scope(|ui| {
                let v = ui.visuals_mut();
                v.widgets.inactive.bg_fill = T.hover;
                v.selection.bg_fill = T.accent;
                ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).show_value(false))
            }).inner.named(name);
            let num = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).suffix(" %").max_decimals(0).speed(0.5))).named(name);
            let r = bar.union(num);
            if r.on_hover_text(tip).changed() {
                c[3] = (pct.clamp(0.0, 100.0) / 100.0 * 255.0).round() as u8;
                changed = true;
            }
        });
    }
    changed
}

/// The button at the right of the picker's top row (#437): on the palette it shows a colour
/// wheel and opens the mixer; on the mixer it shows a grid of swatches and goes back.
fn view_toggle(ui: &mut egui::Ui, view: View) -> Response {
    let size = ui.spacing().interact_size.y;
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let (name, tip) = match view {
        View::Swatches => (tr!("Mixer"), tr!("Mixer: any colour, by hue, saturation and brightness")),
        View::Mixer => (tr!("Swatches"), tr!("Swatches: the web-safe palette, the colours used lately and the document's swatches")),
    };
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if resp.hovered() || resp.has_focus() {
            p.rect_filled(rect, T.rc(), T.hover);
        }
        let c = rect.center();
        match view {
            View::Swatches => {
                // A wheel of the twelve hues, with the panel showing through its hub.
                const HUES: usize = 12;
                let (outer, inner) = (size * 0.34, size * 0.12);
                let at = |i: usize, r: f32| {
                    let a = i as f32 / HUES as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    c + vec2(a.cos(), a.sin()) * r
                };
                for i in 0..HUES {
                    let hue: Color32 = egui::ecolor::Hsva::new(i as f32 / HUES as f32, 0.85, 1.0, 1.0).into();
                    p.add(egui::Shape::convex_polygon(vec![at(i, inner), at(i, outer), at(i + 1, outer), at(i + 1, inner)], hue, Stroke::NONE));
                }
                p.circle_stroke(c, outer, Stroke::new(1.0, T.border));
            }
            View::Mixer => {
                // Three by three swatches.
                let (cell, gap) = (size * 0.17, size * 0.06);
                let ink = if resp.hovered() { T.icon_hot } else { T.icon };
                for row in -1..=1 {
                    for col in -1..=1 {
                        let centre = c + vec2(col as f32, row as f32) * (cell + gap);
                        p.rect_filled(Rect::from_center_size(centre, vec2(cell, cell)), 1.0, ink);
                    }
                }
            }
        }
    }
    crate::a11y::control(ui, &resp, egui::Role::Button, name, None);
    resp.on_hover_text(tip)
}

/// The hex field: shows `shown` (the colour, or the swatch under the pointer), and takes a
/// typed colour as soon as it reads as one.
fn hex_field(ui: &mut egui::Ui, popup: Id, c: &[u8; 4], shown: [u8; 4], alpha: bool) -> Option<[u8; 4]> {
    let buf_id = popup.with("hex");
    let current = to_hex(shown, alpha);
    let mut s: String = ui.data(|d| d.get_temp(buf_id)).unwrap_or_else(|| current.clone());
    let r = with_type(ui, Type::Data, |ui| ui.add(egui::TextEdit::singleline(&mut s).desired_width(if alpha { 86.0 } else { 66.0 }).char_limit(10)));
    let r = r.named(tr!("Hex colour")).on_hover_text(if alpha { tr!("Hex colour: #rrggbb, or #rrggbbaa with alpha") } else { tr!("Hex colour: #rrggbb") });
    let mut out = None;
    if r.has_focus() || r.lost_focus() {
        if r.changed()
            && let Some(p) = parse_hex(&s, alpha)
            && p != *c
        {
            out = Some(p);
        }
        if r.lost_focus() {
            // Back to the colour it holds (an unreadable entry is dropped).
            ui.data_mut(|d| d.remove::<String>(buf_id));
        } else {
            ui.data_mut(|d| d.insert_temp(buf_id, s));
        }
    } else {
        ui.data_mut(|d| d.remove::<String>(buf_id));
    }
    out
}

/// The document's solid swatches (ADR 0078), as the picker's last rows show them: colour and
/// name.
fn document_colours(ctx: &egui::Context) -> Vec<([u8; 3], String)> {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    crate::swatches_panel::for_pickers(ctx)
        .iter()
        .filter_map(|s| match &s.paint {
            anicraft_engine::doc::SwatchPaint::Solid { color } => Some(([b(color.r), b(color.g), b(color.b)], s.name.clone())),
            _ => None,
        })
        .take((COLS + 1) * DOC_ROWS)
        .collect()
}

/// The palette grid (side column, a gap, the web-safe cube), the recent row under it, and the
/// document's swatches under that. Returns the colour clicked, and the one under the pointer.
fn swatches(ui: &mut egui::Ui, current: [u8; 3]) -> (Option<[u8; 3]>, Option<[u8; 3]>) {
    let recent: Vec<[u8; 3]> = ui.data(|d| d.get_temp::<Recent>(recent_id())).map(|r| r.colours).unwrap_or_default();
    let doc = document_colours(ui.ctx());
    let doc_rows = doc.len().div_ceil(COLS + 1);
    let gap = CELL * 0.5;
    let width = CELL * (COLS + 1) as f32 + gap;
    let height = CELL * (ROWS + 1) as f32 + gap + if doc_rows > 0 { gap + CELL * (doc_rows + 1) as f32 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let painter = ui.painter_at(rect.expand(2.0));
    let o = rect.min;
    // Each cell: where it is, and its colour.
    let mut cells: Vec<(Rect, [u8; 3])> = Vec::with_capacity(ROWS * (COLS + 1) + RECENT);
    for (row, side) in SIDE.iter().enumerate() {
        let y = o.y + CELL * row as f32;
        cells.push((Rect::from_min_size(pos2(o.x, y), vec2(CELL, CELL)), *side));
        for col in 0..COLS {
            if let Some(c) = web_safe(row, col) {
                cells.push((Rect::from_min_size(pos2(o.x + CELL + gap + CELL * col as f32, y), vec2(CELL, CELL)), c));
            }
        }
    }
    let ry = o.y + CELL * ROWS as f32 + gap;
    for (i, c) in recent.iter().enumerate().take(RECENT) {
        cells.push((Rect::from_min_size(pos2(o.x + CELL * i as f32 + if i > 0 { gap } else { 0.0 }, ry), vec2(CELL, CELL)), *c));
    }
    // The document's swatches under their caption, a row at a time (names show on hover).
    let dy = ry + CELL + gap + CELL;
    let first_doc = cells.len();
    for (i, (c, _)) in doc.iter().enumerate() {
        cells.push((Rect::from_min_size(pos2(o.x + CELL * (i % (COLS + 1)) as f32, dy + CELL * (i / (COLS + 1)) as f32), vec2(CELL, CELL)), *c));
    }
    // The palette's own grid lines are the gaps between cells, in the panel's border colour.
    let grid = Rect::from_min_size(o, vec2(width, CELL * ROWS as f32));
    painter.rect_filled(grid, 0.0, T.border);
    for (r, c) in &cells {
        painter.rect_filled(r.shrink(0.5), 0.0, c32([c[0], c[1], c[2], 255]));
    }
    if !doc.is_empty() {
        painter.text(pos2(o.x, dy - CELL * 0.5), egui::Align2::LEFT_CENTER, tr!("Document swatches (Window › Swatches)"), crate::theme::font(Type::Caption), T.text_faint);
    }
    if recent.is_empty() {
        painter.text(pos2(o.x, ry + CELL * 0.5), egui::Align2::LEFT_CENTER, tr!("Colours you mix or type show here"), crate::theme::font(Type::Caption), T.text_faint);
    }
    let hit = |p: Pos2| cells.iter().find(|(r, _)| r.contains(p)).copied();
    if let Some((r, _)) = cells.iter().find(|(_, c)| *c == current) {
        painter.rect_stroke(r.expand(1.0), 0.0, Stroke::new(1.5, T.on_active), StrokeKind::Middle);
    }
    let hovered = resp.hover_pos().and_then(|p| cells.iter().position(|(r, _)| r.contains(p)));
    if let Some((r, c)) = hovered.and_then(|i| cells.get(i)).copied() {
        painter.rect_stroke(r.expand(1.0), 0.0, Stroke::new(1.5, T.selection), StrokeKind::Middle);
        let hex = to_hex([c[0], c[1], c[2], 255], false);
        let name = hovered.and_then(|i| i.checked_sub(first_doc)).and_then(|i| doc.get(i)).map(|d| d.1.as_str()).filter(|n| !n.is_empty());
        let tip = match name {
            Some(n) => tr!("{name} ({hex}), a document swatch", name = n, hex = hex.as_str()),
            None if hovered.is_some_and(|i| i >= first_doc) => tr!("{hex}, a document swatch", hex = hex.as_str()),
            None => hex,
        };
        resp.clone().on_hover_text(text(Type::Data, tip));
    }
    // One control for the whole grid (a click picks the cell under the pointer).
    crate::a11y::control(ui, &resp, egui::Role::Button, if doc.is_empty() { tr!("Colour swatches") } else { tr!("Colour swatches and the document's swatches") }, None);
    let over = hovered.and_then(|i| cells.get(i)).map(|(_, c)| *c);
    (if resp.clicked() { resp.interact_pointer_pos().and_then(hit).map(|(_, c)| c) } else { None }, over)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_short_long_and_alpha_forms() {
        assert_eq!(parse_hex("#ff8000", false), Some([255, 128, 0, 255]));
        assert_eq!(parse_hex(" FF8000 ", false), Some([255, 128, 0, 255]));
        assert_eq!(parse_hex("#f80", false), Some([255, 136, 0, 255]));
        assert_eq!(parse_hex("#ff800080", true), Some([255, 128, 0, 128]));
        assert_eq!(parse_hex("#f808", true), Some([255, 136, 0, 136]));
        for bad in ["", "#", "#ff80", "#ff800080", "##ff8000", "#gg0000", "#+f0000", "#ff 800", "#ff8000ff00"] {
            assert_eq!(parse_hex(bad, false), None, "{bad:?}");
        }
        assert_eq!(parse_hex("#ff80008", true), None);
        // What the field shows reads back as the same colour.
        for c in [[0, 0, 0, 255], [18, 52, 86, 255], [255, 255, 255, 7]] {
            assert_eq!(parse_hex(&to_hex(c, true), true), Some(c));
        }
        assert_eq!(to_hex([1, 2, 3, 4], false), "#010203", "no alpha where the colour has none");
    }

    /// #248: the 216 web-safe colours, each once, black at the top left and white where the
    /// last block ends (the open standard's cube).
    #[test]
    fn the_palette_is_the_web_safe_cube() {
        let mut all: Vec<[u8; 3]> = (0..ROWS).flat_map(|r| (0..COLS).filter_map(move |c| web_safe(r, c))).collect();
        assert_eq!(all.len(), 216);
        assert!(all.iter().all(|c| c.iter().all(|v| STEPS.contains(v))));
        assert_eq!(web_safe(0, 0), Some([0, 0, 0]));
        assert_eq!(web_safe(11, 17), Some([255, 255, 255]));
        assert_eq!(web_safe(0, 6), Some([0x33, 0, 0]), "red steps across the blocks");
        // #432: the order artists know: green across a block, blue down it.
        assert_eq!(web_safe(0, 1), Some([0, 0x33, 0]), "green across a block");
        assert_eq!(web_safe(1, 0), Some([0, 0, 0x33]), "blue down it");
        assert_eq!(web_safe(0, 5), Some([0, 0xff, 0]), "the first block's top right is green");
        assert_eq!(web_safe(5, 0), Some([0, 0, 0xff]), "and its bottom left is blue");
        assert_eq!(web_safe(6, 0), Some([0x99, 0, 0]), "the second row of blocks goes on with red");
        assert_eq!(web_safe(6, 17), Some([0xff, 0xff, 0]), "yellow ends the first row of the last block");
        assert_eq!(web_safe(12, 0), None);
        assert_eq!(web_safe(0, 18), None);
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 216, "no colour twice");
    }

    /// #434, #435, #437: the top row shows the swatch under the pointer, as a colour and as hex,
    /// without changing the colour until a click; one button goes to the mixer and back.
    #[test]
    fn the_top_row_follows_the_pointer_and_one_button_swaps_palette_and_mixer() {
        use egui::accesskit::{Node, TreeUpdate};
        use egui::{Event, PointerButton, Pos2};
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        crate::theme::apply(&ctx);
        let id = Id::unique("fill_popup");
        let mut c = [255, 255, 255, 255];
        let frame = |ctx: &egui::Context, c: &mut [u8; 4], events: Vec<Event>| -> (bool, TreeUpdate) {
            let mut changed = false;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 500.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| changed = picker(ui, id, c, false, None, true));
            out.textures_delta.clear();
            (changed, out.platform_output.accesskit_update.expect("the accessibility tree"))
        };
        fn find<'a>(t: &'a TreeUpdate, label: &str) -> Option<&'a Node> {
            t.nodes.iter().map(|(_, n)| n).find(|n| n.label() == Some(label))
        }
        let centre = |n: &Node| n.bounds().map(|b| Pos2::new(((b.x0 + b.x1) * 0.5) as f32, ((b.y0 + b.y1) * 0.5) as f32)).expect("it is on screen");
        let click = |ctx: &egui::Context, c: &mut [u8; 4], at: Pos2| -> bool {
            let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            frame(ctx, c, vec![Event::PointerMoved(at)]);
            frame(ctx, c, vec![button(true)]);
            let changed = frame(ctx, c, vec![button(false)]).0;
            changed | frame(ctx, c, Vec::new()).0
        };
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        let (_, t) = (t, frame(&ctx, &mut c, Vec::new()).1);
        assert_eq!(find(&t, "Hex colour").and_then(Node::value), Some("#ffffff"));
        let (hex, grid) = (find(&t, "Hex colour").map(centre).unwrap(), find(&t, "Colour swatches").expect("the palette"));
        let top_left = grid.bounds().map(|b| Pos2::new(b.x0 as f32, b.y0 as f32)).unwrap();
        assert!(hex.y < top_left.y, "the hex field is above the palette");
        // The first block's top right cell (pure green): the side column, the gap, six cells.
        let green = top_left + vec2(CELL + CELL * 0.5 + CELL * 5.5, CELL * 0.5);
        frame(&ctx, &mut c, vec![Event::PointerMoved(green)]);
        let (changed, t) = frame(&ctx, &mut c, Vec::new());
        assert_eq!(find(&t, "Hex colour").and_then(Node::value), Some("#00ff00"), "the hex follows the pointer");
        assert!(!changed && c == [255, 255, 255, 255], "looking changes nothing");
        // Off the palette: back to the colour itself.
        frame(&ctx, &mut c, vec![Event::PointerMoved(hex)]);
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        assert_eq!(find(&t, "Hex colour").and_then(Node::value), Some("#ffffff"));
        // A click takes it.
        assert!(click(&ctx, &mut c, green));
        assert_eq!(c, [0, 255, 0, 255]);
        // One button: to the mixer (the palette goes), and back.
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        assert!(find(&t, "Swatches").is_none(), "on the palette the button offers the mixer");
        let toggle = find(&t, "Mixer").map(centre).expect("the mixer button");
        assert!(toggle.x > hex.x && toggle.y < top_left.y, "at the right of the top row");
        click(&ctx, &mut c, toggle);
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        assert!(find(&t, "Colour swatches").is_none() && find(&t, "Mixer").is_none());
        let back = find(&t, "Swatches").map(centre).expect("the way back to the swatches");
        assert_eq!(find(&t, "Hex colour").and_then(Node::value), Some("#00ff00"), "the mixer keeps the top row");
        click(&ctx, &mut c, back);
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        assert!(find(&t, "Colour swatches").is_some());
        ctx.data_mut(|d| d.remove::<View>(Id::unique("anicraft.colour.view")));
    }

    /// #570: a swatch clicked in a colour button's picker is the choice: the colour changes and
    /// the picker closes.
    #[test]
    fn a_swatch_picked_closes_the_picker() {
        use egui::accesskit::TreeUpdate;
        use egui::{Event, PointerButton, Pos2};
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        crate::theme::apply(&ctx);
        let mut c = [255, 255, 255];
        let frame = |ctx: &egui::Context, c: &mut [u8; 3], events: Vec<Event>| -> (bool, TreeUpdate) {
            let mut changed = false;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 600.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| changed = button(ui, c).changed());
            out.textures_delta.clear();
            (changed, out.platform_output.accesskit_update.expect("the accessibility tree"))
        };
        let bounds = |t: &TreeUpdate, label: &str| t.nodes.iter().map(|(_, n)| n).find(|n| n.label() == Some(label)).and_then(|n| n.bounds());
        let click = |ctx: &egui::Context, c: &mut [u8; 3], at: Pos2| -> bool {
            let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            frame(ctx, c, vec![Event::PointerMoved(at)]);
            frame(ctx, c, vec![button(true)]);
            let changed = frame(ctx, c, vec![button(false)]).0;
            changed | frame(ctx, c, Vec::new()).0
        };
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        let swatch = t.nodes.iter().map(|(_, n)| n).find(|n| n.role() == egui::accesskit::Role::ColorWell).and_then(|n| n.bounds()).expect("the colour button");
        click(&ctx, &mut c, Pos2::new(((swatch.x0 + swatch.x1) * 0.5) as f32, ((swatch.y0 + swatch.y1) * 0.5) as f32));
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        let grid = bounds(&t, "Colour swatches").expect("the picker opened");
        // The first block's top right cell (pure green): the side column, the gap, six cells.
        let green = Pos2::new(grid.x0 as f32, grid.y0 as f32) + vec2(CELL + CELL * 0.5 + CELL * 5.5, CELL * 0.5);
        assert!(click(&ctx, &mut c, green), "the colour changed");
        assert_eq!(c, [0, 255, 0]);
        let (_, t) = frame(&ctx, &mut c, Vec::new());
        assert!(bounds(&t, "Colour swatches").is_none(), "the picker closed");
    }

    #[test]
    fn recent_colours_keep_one_entry_per_popup_session() {
        let ctx = egui::Context::default();
        let (a, b) = (Id::unique("a"), Id::unique("b"));
        let list = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<Recent>(recent_id())).map(|r| r.colours).unwrap_or_default();
        // A drag in the mixer: one entry, the last colour.
        for v in [10, 20, 30] {
            remember(&ctx, a, [v, 0, 0]);
        }
        assert_eq!(list(&ctx), vec![[30, 0, 0]]);
        // Another popup adds one, newest first; the same colour again moves to the front.
        remember(&ctx, b, [0, 0, 9]);
        new_session(&ctx);
        remember(&ctx, b, [30, 0, 0]);
        assert_eq!(list(&ctx), vec![[30, 0, 0], [0, 0, 9]]);
        new_session(&ctx);
        for v in 0..40u8 {
            new_session(&ctx);
            remember(&ctx, a, [v, v, v]);
        }
        assert_eq!(list(&ctx).len(), RECENT, "bounded");
    }

    #[test]
    fn a_click_on_the_stage_gives_the_open_picker_its_colour_once() {
        let ctx = egui::Context::default();
        let (a, b) = (Id::unique("fill_popup"), Id::unique("stroke_popup"));
        let mut c = [10, 20, 30, 128];
        let frame = |ctx: &egui::Context, id: Id, c: &mut [u8; 4]| {
            let mut changed = false;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| changed = picker(ui, id, c, true, None, true));
            out.textures_delta.clear();
            changed
        };
        assert_eq!(picking(&ctx), None, "no picker, no eyedropper");
        assert_eq!(close_behavior(&ctx), egui::PopupCloseBehavior::CloseOnClickOutside);
        // The picker shows: the stage is its eyedropper, and nothing has changed.
        assert!(!frame(&ctx, a, &mut c));
        assert_eq!(picking(&ctx), Some(a));
        // The pointer over the stage keeps the popup open through the click that picks.
        over_stage(&ctx);
        assert_eq!(close_behavior(&ctx), egui::PopupCloseBehavior::IgnoreClicks);
        // The stage hands over the colour clicked: the picker takes it, alpha kept, once.
        pick_from_stage(&ctx, a, [200, 100, 50]);
        assert!(frame(&ctx, a, &mut c));
        assert_eq!(c, [200, 100, 50, 128]);
        assert!(ctx.data(|d| d.get_temp::<Recent>(recent_id())).is_some_and(|r| r.colours.first() == Some(&[200, 100, 50])), "and it joins the colours used lately");
        assert!(!frame(&ctx, a, &mut c), "taken once");
        // Another picker doesn't take a colour meant for this one, and one never collected
        // (the picker closed first) is dropped rather than turning up later.
        pick_from_stage(&ctx, a, [1, 2, 3]);
        let mut other = [9, 9, 9, 255];
        assert!(!frame(&ctx, b, &mut other));
        assert_eq!(other, [9, 9, 9, 255]);
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
            out.textures_delta.clear();
        }
        assert_eq!(picking(&ctx), None, "the picker closed: the stage is the tools' again");
        assert_eq!(close_behavior(&ctx), egui::PopupCloseBehavior::CloseOnClickOutside);
        assert!(!frame(&ctx, a, &mut c));
        assert_eq!(c, [200, 100, 50, 128], "the stale colour is gone");
    }
}
