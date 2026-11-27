//! The fill side of the colour picker (ADR 0074): one picker, as Animate has one Color panel.
//!
//! The Tools panel's Fill square and every Fill swatch in Properties open the same popup. Its
//! top row is the fill's type (None, Solid, Linear, Radial, Bitmap). A solid fill shows the
//! colour picker (`crate::colour`: web-safe swatches, the mixer, hex). A gradient shows its stop
//! bar, overflow, Linear RGB and focal point above that same picker, which then edits the picked
//! stop's colour and opacity. A bitmap fill shows the Library image it uses. The Stroke square
//! opens the colour picker alone (strokes are one colour).
//!
//! The popup says what was asked ([`FillEdit`]); its caller applies it: a solid colour or None
//! the way that place always has, anything else as `paint.set_fill`, which sets the fill swatch
//! and the selected fills in one step, so agents and the CLI work the same way.

use crate::a11y::{self, Named};
use crate::app::App;
use crate::theme::{T, Type, text, to_color32, with_type};
use anicraft_engine::ArrayPropertyStop;
use anicraft_engine::doc::{ElementKind, GradientStop, MAX_FOCAL, MAX_GRADIENT_STOPS, Paint, Rgba, Spread, ramp_at};
use egui::{Id, Key, Rect, Response, Sense, Stroke, StrokeKind, Role, pos2, vec2};
use serde_json::{Value, json};

/// How far (screen points) a stop dragged off the bar must go to be removed.
const TEAR_OFF: f32 = 26.0;
/// The stop bar's width: the colour picker's swatch grid under it.
const BAR_WIDTH: f32 = 270.0;

fn focused_stop_id() -> Id { Id::unique("focused_stop_handle") }

pub(crate) fn stop_has_focus(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Id>(focused_stop_id())).is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
}

/// What the fill picker asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum FillEdit {
    /// A solid colour, unmultiplied, alpha in the last byte. 0 is a clear fill, not "no fill".
    Solid([u8; 4]),
    /// No fill (true), or a fill again (false).
    None(bool),
    /// `paint.set_fill` params, with its undo label; `live` while a drag or typing goes on (one
    /// undo step for all of it).
    Set { label: &'static str, live: bool, params: Value },
}

/// The fill the pickers show: the first selected shape's on the active layer, else the swatch.
/// True when it is the selection's.
pub fn current(app: &App) -> (Paint, bool) {
    let selected = app.s.selected_elements().into_iter().filter(|(l, _)| *l == app.s.layer).find_map(|(_, e)| match &e.kind {
        ElementKind::Shape(sh) => sh.fill.clone(),
        _ => None,
    });
    match selected {
        Some(p) => (p, true),
        None => (app.s.style.fill_paint(), false),
    }
}

/// The Library's bitmaps, for a bitmap fill: `(id, name)`.
pub fn bitmaps(app: &App) -> Vec<(u64, String)> {
    app.s.doc.bitmaps.iter().map(|b| (b.id.0, b.name.clone())).collect()
}

/// Applies `edit` as `paint.set_fill` (`selection`: also to the selected fills). Solid and None
/// edits are the caller's (each place keeps its own rule for those).
pub fn apply_set(app: &mut App, edit: &FillEdit, selection: bool) {
    if let FillEdit::Set { label, live, params } = edit {
        let mut p = params.clone();
        if !selection && let Some(o) = p.as_object_mut() {
            o.insert("selection".into(), json!(false));
        }
        if *live {
            crate::panels::live(app, label, "paint.set_fill", p);
        } else {
            app.run("paint.set_fill", p);
        }
    }
}

/// `#rrggbb`, or `#rrggbbaa` when not opaque.
fn hex(c: Rgba) -> String {
    c.to_hex_alpha()
}

fn stops_json(stops: &[GradientStop]) -> Value {
    json!(stops.iter().map(|s| json!({"offset": s.offset.clamp(0.0, 1.0), "color": hex(s.color)})).collect::<Vec<_>>())
}

fn bytes(c: Rgba) -> [u8; 4] {
    let c = c.sanitized();
    let b = |v: f32| (v * 255.0).round() as u8;
    [b(c.r), b(c.g), b(c.b), b(c.a)]
}

fn from_bytes(c: [u8; 4]) -> Rgba {
    Rgba::new(f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, f32::from(c[3]) / 255.0)
}

/// Paints `paint` into `r`: a colour, a gradient's ramp, a bitmap's mark, or the empty field
/// with a slash for no fill.
fn paint_preview(p: &egui::Painter, r: Rect, paint: Option<&Paint>) {
    match paint {
        None => {
            p.rect_filled(r, 0.0, T.field);
            p.line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.5, T.danger));
        }
        Some(Paint::Solid(c)) => egui::color_picker::show_color_at(p, to_color32(*c), r),
        Some(Paint::Bitmap { .. }) => {
            p.rect_filled(r, 0.0, T.text_faint);
            crate::icons::paint(p, r.shrink(2.0), crate::icons::Icon::Image, T.panel);
        }
        Some(g) => {
            let stops = g.stops().unwrap_or(&[]);
            let mut mesh = egui::Mesh::default();
            let steps = 24u32;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let c = to_color32(ramp_at(stops, t));
                let x = r.left() + t * r.width();
                mesh.colored_vertex(pos2(x, r.top()), c);
                mesh.colored_vertex(pos2(x, r.bottom()), c);
                if i > 0 {
                    let k = 2 * i;
                    mesh.add_triangle(k - 2, k - 1, k);
                    mesh.add_triangle(k - 1, k, k + 1);
                }
            }
            p.add(mesh);
        }
    }
}

/// A fill swatch for Properties: shows the fill (a gradient as its ramp) and opens the fill
/// picker. Name it with `Named::named`.
pub fn swatch(ui: &mut egui::Ui, paint: Option<&Paint>, bitmaps: &[(u64, String)]) -> (Response, Option<FillEdit>) {
    let size = ui.spacing().interact_size;
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::new(Role::ColorWell));
    let popup = resp.id.with("colour_popup");
    let open = egui::Popup::is_id_open(ui.ctx(), popup);
    if ui.is_rect_visible(rect) {
        paint_preview(ui.painter(), rect.shrink(1.0), paint);
        ui.painter().rect_stroke(rect, T.rc(), Stroke::new(1.0, if open || resp.hovered() { T.selection } else { T.border }), StrokeKind::Inside);
    }
    let edit = popup_for(ui, &resp, popup, paint, bitmaps);
    if edit.is_some() {
        resp.mark_changed();
    }
    (resp, edit)
}

/// Opens the fill picker under `anchor` when it is clicked, and shows it while open. `paint`
/// is the fill (`None`: no fill).
pub fn popup_for(ui: &egui::Ui, anchor: &Response, popup: Id, paint: Option<&Paint>, bitmaps: &[(u64, String)]) -> Option<FillEdit> {
    if anchor.clicked() {
        crate::colour::new_session(ui.ctx());
    }
    let mut edit = None;
    // A click on the stage while this is open picks a colour there (the picker's eyedropper).
    egui::Popup::menu(anchor).id(popup).close_behavior(crate::colour::close_behavior(ui.ctx())).show(|ui| {
        crate::colour::name_popup(ui, tr!("Fill colour picker"));
        edit = fill_picker(ui, popup, paint, bitmaps);
    });
    edit
}

/// The kinds of fill on the type row, as Animate's Color Type menu: key, label, what it is.
fn kinds() -> [(&'static str, &'static str, &'static str); 5] {
    [
        ("none", tr!("None"), tr!("No fill")),
        ("solid", tr!("Solid"), tr!("A solid colour, from clear to opaque")),
        ("linear", tr!("Linear"), tr!("Linear gradient: colours along a line")),
        ("radial", tr!("Radial"), tr!("Radial gradient: colours out from a centre")),
        ("bitmap", tr!("Bitmap"), tr!("Bitmap fill: a Library image, tiled")),
    ]
}

/// The picker's body: the type row, then what that type has.
pub fn fill_picker(ui: &mut egui::Ui, popup: Id, paint: Option<&Paint>, bitmaps: &[(u64, String)]) -> Option<FillEdit> {
    let now = match paint {
        None => "none",
        Some(Paint::Solid(_)) => "solid",
        Some(Paint::Linear { .. }) => "linear",
        Some(Paint::Radial { .. }) => "radial",
        Some(Paint::Bitmap { .. }) => "bitmap",
    };
    let mut pick = now;
    ui.horizontal(|ui| {
        for (key, label, tip) in kinds() {
            let usable = key != "bitmap" || !bitmaps.is_empty();
            let r = ui.add_enabled(usable, egui::Button::selectable(pick == key, label)).named(&tr!("Fill type: {kind}", kind = tip));
            let r = if usable { r.on_hover_text(tip) } else { r.on_disabled_hover_text(tr!("Import an image into the Library first (File › Import › Import to Library), then fill with it.")) };
            if r.clicked() {
                pick = key;
            }
        }
    });
    ui.add_space(4.0);
    // The document's gradient swatches (ADR 0078): a click fills with one.
    if let Some(edit) = gradient_swatches(ui, paint) {
        return Some(edit);
    }
    if pick != now {
        return Some(match pick {
            "none" => FillEdit::None(true),
            "solid" => match paint {
                // A fill again: the colour it shows.
                None => FillEdit::None(false),
                Some(p) => FillEdit::Set { label: tr_noop!("Fill"), live: false, params: json!({"type": "solid", "color": hex(p.primary_color())}) },
            },
            "bitmap" => FillEdit::Set { label: tr_noop!("Fill"), live: false, params: json!({"type": "bitmap", "bitmap": bitmaps.first().map_or(0, |b| b.0)}) },
            other => FillEdit::Set { label: tr_noop!("Fill"), live: false, params: json!({"type": other}) },
        });
    }
    match paint {
        None => {
            ui.label(text(Type::Caption, tr!("No fill. Choose a type above to fill again.")));
            None
        }
        Some(Paint::Solid(c)) => {
            let mut px = bytes(*c);
            crate::colour::picker(ui, popup, &mut px, true, None, true).then_some(FillEdit::Solid(px))
        }
        Some(Paint::Linear { stops, spread, linear_rgb, .. }) => gradient(ui, popup, stops, *spread, *linear_rgb, None),
        Some(Paint::Radial { stops, spread, linear_rgb, focal, .. }) => gradient(ui, popup, stops, *spread, *linear_rgb, Some(*focal)),
        Some(Paint::Bitmap { bitmap, .. }) => {
            let current = bitmap.0;
            let mut want = current;
            ui.horizontal(|ui| {
                ui.label(tr!("Bitmap"));
                egui::ComboBox::from_id_salt(popup.with("fill_bitmap"))
                    .width(180.0)
                    .selected_text(bitmaps.iter().find(|c| c.0 == current).map_or_else(|| tr!("missing bitmap").to_string(), |c| c.1.clone()))
                    .show_ui(ui, |ui| {
                        for (id, name) in bitmaps {
                            ui.selectable_value(&mut want, *id, name.as_str());
                        }
                    })
                    .response
                    .set_name(tr!("Fill bitmap"));
            });
            ui.label(text(Type::Caption, tr!("Scale, turn and skew it with the Gradient Transform tool (F).")));
            (want != current).then(|| FillEdit::Set { label: tr_noop!("Fill"), live: false, params: json!({"type": "bitmap", "bitmap": want}) })
        }
    }
}

/// The document's gradient swatches as a row (two at most; the Swatches panel has them all).
/// The fill a click asks for.
fn gradient_swatches(ui: &mut egui::Ui, paint: Option<&Paint>) -> Option<FillEdit> {
    const CELL: f32 = 16.0;
    const PER_ROW: usize = 16;
    let all = crate::swatches_panel::for_pickers(ui.ctx());
    let grads: Vec<&anicraft_engine::doc::Swatch> = all.iter().filter(|s| s.paint.is_gradient()).take(PER_ROW * 2).collect();
    if grads.is_empty() {
        return None;
    }
    let mut out = None;
    ui.label(text(Type::Caption, tr!("Gradient swatches")));
    for row in grads.chunks(PER_ROW) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for sw in row {
                let (r, resp) = ui.allocate_exact_size(vec2(CELL, CELL), Sense::click());
                crate::swatches_panel::preview(ui.painter(), r, &sw.paint);
                let on = paint.is_some_and(|p| crate::swatches_panel::matches(&sw.paint, p));
                let edge = if on { Stroke::new(2.0, T.accent) } else if resp.hovered() { Stroke::new(1.5, T.selection) } else { Stroke::new(1.0, T.border) };
                ui.painter().rect_stroke(r, 0.0, edge, StrokeKind::Outside);
                let name = crate::swatches_panel::describe(&sw.paint, &sw.name);
                a11y::control(ui, &resp, Role::Button, &tr!("Gradient swatch: {name}", name = name.as_str()), Some(on));
                if resp.on_hover_text(name).clicked() {
                    out = Some(FillEdit::Set { label: tr_noop!("Fill"), live: false, params: crate::swatches_panel::fill_params(&sw.paint) });
                }
            }
        });
    }
    ui.add_space(4.0);
    out
}

/// A gradient: overflow, Linear RGB and the focal point, the stop bar, the picked stop's
/// position, and the colour picker for its colour.
fn gradient(ui: &mut egui::Ui, popup: Id, stops: &[GradientStop], spread: Spread, linear_rgb: bool, focal: Option<f32>) -> Option<FillEdit> {
    let set = |label: &'static str, live: bool, params: Value| Some(FillEdit::Set { label, live, params });
    let mut out = None;
    let mut sp = spread;
    ui.horizontal(|ui| {
        ui.label(tr!("Overflow"));
        for (s, label, tip) in [
            (Spread::Pad, tr!("Extend"), tr!("Past its ends the gradient keeps its end colours")),
            (Spread::Reflect, tr!("Reflect"), tr!("Past its ends the gradient mirrors back and forth")),
            (Spread::Repeat, tr!("Repeat"), tr!("Past its ends the gradient starts again")),
        ] {
            ui.selectable_value(&mut sp, s, label).on_hover_text(tip);
        }
    });
    if sp != spread {
        out = set(tr_noop!("Overflow"), false, json!({"spread": sp.key()}));
    }
    ui.horizontal(|ui| {
        let mut lin = linear_rgb;
        if crate::theme::checkbox(ui, &mut lin, tr!("Linear RGB")).on_hover_text(tr!("Mix the colours in linear light: brighter, more even blends")).changed() {
            out = set(tr_noop!("Linear RGB"), false, json!({"linear_rgb": lin}));
        }
        if let Some(f) = focal {
            ui.add_space(8.0);
            ui.label(tr!("Focal point"));
            let mut pct = f * 100.0;
            let lim = MAX_FOCAL * 100.0;
            let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut pct).suffix(" %").speed(0.5).range(-lim..=lim).max_decimals(0)));
            if r.named(tr!("Focal point")).on_hover_text(tr!("Where the ramp starts, from the centre along the gradient's axis (drag the triangle with the Gradient Transform tool)")).changed() {
                out = set(tr_noop!("Focal point"), true, json!({"focal": (pct / 100.0).clamp(-MAX_FOCAL, MAX_FOCAL)}));
            }
        }
    });
    ui.add_space(4.0);
    let mut edited = stops.to_vec();
    if let Some((label, live)) = stop_bar(ui, popup, &mut edited) {
        out = set(label, live, json!({"stops": stops_json(&edited)}));
    }
    out
}

trait TrackStop: Clone + PartialEq {
    const ARRAY: bool;
    const LIMIT: usize;
    fn offset(&self) -> f32;
    fn set_offset(&mut self, offset: f32);
    fn alpha(&self) -> f32;
    fn colour(&self) -> Option<Rgba>;
    fn sample(stops: &[Self], offset: f32) -> Self;
    fn marker(&self, index: usize) -> usize { index }
}

impl TrackStop for GradientStop {
    const ARRAY: bool = false;
    const LIMIT: usize = MAX_GRADIENT_STOPS;
    fn offset(&self) -> f32 { self.offset }
    fn set_offset(&mut self, offset: f32) { self.offset = offset; }
    fn alpha(&self) -> f32 { self.color.sanitized().a }
    fn colour(&self) -> Option<Rgba> { Some(self.color) }
    fn sample(stops: &[Self], offset: f32) -> Self { Self { offset, color: ramp_at(stops, offset) } }
}

/// An Array stop has a stable marker so it stays grabbed when it crosses another stop.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PropertyStop {
    marker: usize,
    pub value: ArrayPropertyStop,
}

impl PropertyStop {
    pub(crate) fn new(marker: usize, offset: f32, alpha: f32) -> Self {
        Self { marker, value: ArrayPropertyStop::new(offset, alpha) }
    }

    pub(crate) fn marker_id(&self) -> usize { self.marker }
}

impl TrackStop for PropertyStop {
    const ARRAY: bool = true;
    const LIMIT: usize = ArrayPropertyStop::LIMIT;
    fn offset(&self) -> f32 { self.value.offset }
    fn set_offset(&mut self, offset: f32) { self.value.offset = offset; }
    fn alpha(&self) -> f32 { self.value.alpha }
    fn colour(&self) -> Option<Rgba> { None }
    fn sample(stops: &[Self], offset: f32) -> Self {
        let marker = (0..Self::LIMIT).find(|i| stops.iter().all(|s| s.marker != *i)).unwrap_or(Self::LIMIT);
        let next = stops.partition_point(|s| s.offset() <= offset);
        let mut value = stops.get(next.saturating_sub(1)).map_or_else(
            || ArrayPropertyStop::new(offset, 1.0),
            |left| left.value.clone(),
        );
        value.offset = offset;
        Self { marker, value }
    }
    fn marker(&self, _: usize) -> usize { self.marker }
}

/// For gradient stops, keep a pointer drag or a key nudge between its neighbours.
fn room(stops: &[impl TrackStop], i: usize) -> (f32, f32) {
    let lo = i.checked_sub(1).and_then(|j| stops.get(j)).map_or(0.0, TrackStop::offset);
    let hi = stops.get(i + 1).map_or(1.0, TrackStop::offset);
    (lo.min(hi), hi.max(lo))
}

fn move_stop<S: TrackStop>(stops: &mut [S], i: usize, offset: f32) {
    let (lo, hi) = if S::ARRAY { (0.0, 1.0) } else { room(stops, i) };
    if offset.is_finite() && let Some(s) = stops.get_mut(i) {
        s.set_offset(offset.clamp(lo, hi));
    }
}

/// The middle of the widest gap between stops: where the keyboard adds one.
fn widest_gap(stops: &[impl TrackStop]) -> f32 {
    stops.windows(2).filter_map(|w| if let [a, b] = w { Some((b.offset() - a.offset(), (a.offset() + b.offset()) / 2.0)) } else { None }).max_by(|a, b| a.0.total_cmp(&b.0)).map_or(0.5, |g| g.1)
}

/// Adds a stop at `t` using the track's inheritance rule.
fn add_stop<S: TrackStop>(stops: &mut Vec<S>, t: f32) -> Option<usize> {
    if stops.len() >= S::LIMIT {
        return None;
    }
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.5 };
    let i = stops.iter().position(|s| s.offset() > t).unwrap_or(stops.len());
    stops.insert(i, S::sample(stops, t));
    Some(i)
}

/// Removes stop `i` unless only two are left. True when it went.
fn remove_stop(stops: &mut Vec<impl TrackStop>, i: usize) -> bool {
    if stops.len() <= 2 || i >= stops.len() {
        return false;
    }
    stops.remove(i);
    true
}

/// The bar of colour stops, the picked stop's position and its colour (the colour picker,
/// with opacity). What changed (its undo label), and whether it is live (a drag or typing).
fn stop_bar(ui: &mut egui::Ui, popup: Id, stops: &mut Vec<GradientStop>) -> Option<(&'static str, bool)> {
    stop_bar_with(ui, popup, stops, None, |ui, sel, st| {
        let mut px = bytes(st.color);
        if crate::colour::picker(ui, popup.with(("stop", sel)), &mut px, true, None, false) {
            st.color = from_bytes(px);
            true
        } else {
            false
        }
    })
}

pub(crate) fn array_stop_bar(ui: &mut egui::Ui, id: Id, stops: &mut Vec<PropertyStop>, marker: &mut Option<usize>, editor: impl FnOnce(&mut egui::Ui, &mut PropertyStop) -> bool) -> bool {
    let mut selected = stops.iter().position(|s| Some(s.marker) == *marker).unwrap_or(0);
    let changed = stop_bar_with(ui, id, stops, Some(&mut selected), |ui, _, stop| editor(ui, stop)).is_some();
    *marker = stops.get(selected).map(|s| s.marker);
    changed
}

/// Shared stop interaction. Each track paints its own marks and edits its selected payload.
fn stop_bar_with<S: TrackStop>(
    ui: &mut egui::Ui,
    popup: Id,
    stops: &mut Vec<S>,
    selected: Option<&mut usize>,
    editor: impl FnOnce(&mut egui::Ui, usize, &mut S) -> bool,
) -> Option<(&'static str, bool)> {
    let array = S::ARRAY;
    let sel_id = popup.with("fill_stop");
    let tear_id = popup.with("fill_stop_tear");
    let mut sel: usize = selected.as_deref().copied().or_else(|| ui.data(|d| d.get_temp(sel_id)))
        .unwrap_or(0).min(stops.len().saturating_sub(1));
    let mut changed = None;
    let can_remove = stops.len() > 2;
    let w = if array { (ui.available_width() - 6.0).clamp(1.0, BAR_WIDTH) } else { BAR_WIDTH };
    let (strip, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click());
    a11y::control(ui, &resp, Role::Button, if array { tr!("Array: add a property stop") } else { tr!("Gradient: add a colour stop") }, None);
    let resp = resp.on_hover_text(if array { tr!("Click to add a property stop. Drag a stop to move it, or off the track to remove it. With a stop focused, the arrow keys move it and Delete removes it.") } else { tr!("Click to add a colour stop. Drag a stop to move it, or off the bar to remove it. With a stop focused, the arrow keys move it and Delete removes it.") });
    let p = ui.painter();
    if array {
        p.hline(strip.x_range(), strip.center().y, Stroke::new(1.0, T.control_edge));
    } else {
        paint_gradient_strip(p, strip, |t| S::sample(stops, t).colour().map_or(T.header, to_color32));
    }
    // Gradient handles sit under the colour strip. Property handles sit on the neutral line.
    let track = if array { strip } else { ui.allocate_exact_size(vec2(w, 14.0), Sense::hover()).0 };
    let mut remove: Option<usize> = None;
    let mut tearing: Option<usize> = ui.data(|d| d.get_temp(tear_id));
    let count = stops.len();
    for i in 0..count {
        let Some(st) = stops.get(i).cloned() else { continue };
        let off = st.offset();
        let marker = st.marker(i);
        let x = strip.left() + off.clamp(0.0, 1.0) * strip.width();
        let hit = Rect::from_center_size(pos2(x, track.center().y), vec2(12.0, track.height()));
        let r = ui.interact(hit, popup.with(("fill_stop_marker", marker)), Sense::click_and_drag());
        let at = format!("{:.0}", off * 100.0);
        let alpha = format!("{:.0}", st.alpha() * 100.0);
        let accessible_name = if array { tr!("Array stop {stop} of {count} at {at}%, alpha {alpha}%", stop = i + 1, count = count, at = at, alpha = alpha) } else { tr!("Colour stop {stop} of {count} at {at}%, alpha {alpha}%", stop = i + 1, count = count, at = at, alpha = alpha) };
        a11y::control(ui, &r, Role::Button, &accessible_name, Some(i == sel));
        if r.clicked() || r.drag_started() || r.gained_focus() {
            sel = i;
        }
        if r.dragged()
            && let Some(pt) = r.interact_pointer_pos()
        {
            sel = i;
            // Far enough above or below the bar: the stop comes off when let go.
            let off_bar = can_remove && (pt.y - track.center().y).abs() > TEAR_OFF && (pt.y - strip.center().y).abs() > TEAR_OFF;
            tearing = off_bar.then_some(marker);
            if !off_bar {
                move_stop(stops, i, (pt.x - strip.left()) / strip.width());
                changed = Some((if array { tr_noop!("Move array stop") } else { tr_noop!("Move colour stop") }, true));
            }
        }
        if r.drag_stopped() && tearing == Some(marker) {
            remove = Some(i);
            tearing = None;
        }
        // The keyboard: arrows move the focused stop (Shift: by 10 %), Delete removes it.
        if r.has_focus() {
            ui.data_mut(|d| d.insert_temp(focused_stop_id(), r.id));
            let (left, right, shift, del) = ui.input(|inp| (inp.key_pressed(Key::ArrowLeft), inp.key_pressed(Key::ArrowRight), inp.modifiers.shift, inp.key_pressed(Key::Delete) || inp.key_pressed(Key::Backspace)));
            let step = if shift { 0.1 } else { 0.01 };
            let d = if right { step } else if left { -step } else { 0.0 };
            if d != 0.0 {
                move_stop(stops, i, off + d);
                changed = Some((if array { tr_noop!("Move array stop") } else { tr_noop!("Move colour stop") }, true));
            }
            if del && can_remove {
                remove = Some(i);
            }
            // Arrow keys stay on the stop rather than moving focus.
            ui.memory_mut(|m| m.set_focus_lock_filter(r.id, egui::EventFilter { horizontal_arrows: true, ..Default::default() }));
        }
        let torn = tearing == Some(marker);
        let edge = if i == sel { T.accent } else if r.hovered() { T.text } else { T.control_edge };
        let tip = pos2(x, track.top());
        let body = [tip, pos2(x + 6.0, track.top() + 6.0), pos2(x + 6.0, track.bottom()), pos2(x - 6.0, track.bottom()), pos2(x - 6.0, track.top() + 6.0)];
        let fill = if let Some(colour) = st.colour() {
            let shown = colour.sanitized();
            to_color32(shown.with_alpha(shown.a * if torn { 0.35 } else { 1.0 }))
        } else {
            let neutral = if i == sel { T.accent_dim } else { T.panel_alt };
            if torn { neutral.gamma_multiply(0.35) } else { neutral }
        };
        let edge = if array && torn { edge.gamma_multiply(0.35) } else { edge };
        ui.painter().add(egui::Shape::convex_polygon(body.to_vec(), fill, Stroke::new(if i == sel { 2.0 } else { 1.0 }, edge)));
    }
    ui.data_mut(|d| match tearing {
        Some(t) => {
            d.insert_temp(tear_id, t);
        }
        None => d.remove::<usize>(tear_id),
    });
    if resp.clicked() {
        // Where the pointer clicked; from the keyboard or a screen reader, the middle of the
        // widest gap.
        let t = resp.interact_pointer_pos().map_or_else(|| widest_gap(stops), |at| (at.x - strip.left()) / strip.width());
        if let Some(i) = add_stop(stops, t) {
            sel = i;
            changed = Some((if array { tr_noop!("Add array stop") } else { tr_noop!("Add colour stop") }, false));
        }
    }
    if let Some(i) = remove.take()
        && remove_stop(stops, i)
    {
        sel = sel.min(stops.len().saturating_sub(1));
        changed = Some((if array { tr_noop!("Remove array stop") } else { tr_noop!("Remove colour stop") }, false));
    }
    // The picked stop: its position, then its colour in the colour picker.
    let n = stops.len();
    if let Some(st) = stops.get_mut(sel) {
        ui.add_space(2.0);
        let marker = st.marker(sel);
        let position = |ui: &mut egui::Ui| {
            ui.label(tr!("Stop {stop} at", stop = sel + 1));
            let mut pct = st.offset() * 100.0;
            let r = with_type(ui, Type::Data, |ui| ui.add(egui::DragValue::new(&mut pct).suffix(" %").speed(0.5).range(0.0..=100.0).max_decimals(0)));
            if r.named(if array { tr!("Array stop position") } else { tr!("Colour stop position") }).changed() {
                st.set_offset((pct / 100.0).clamp(0.0, 1.0));
                changed = Some((if array { tr_noop!("Move array stop") } else { tr_noop!("Move colour stop") }, true));
            }
            if ui.add_enabled(n > 2, egui::Button::new(tr!("Remove")).small()).named(if array { tr!("Remove array stop") } else { tr!("Remove colour stop") }).on_hover_text(tr!("Remove this stop (or drag it off the bar)")).clicked() {
                remove = Some(sel);
            }
        };
        if array {
            ui.push_id(popup.with(marker), |ui| ui.horizontal_wrapped(position));
        } else {
            ui.horizontal(position);
        }
        ui.add_space(4.0);
        if editor(ui, sel, st) {
            changed = Some((if array { tr_noop!("Array stop properties") } else { tr_noop!("Colour stop") }, true));
        }
    }
    if let Some(i) = remove
        && remove_stop(stops, i)
    {
        sel = sel.min(stops.len().saturating_sub(1));
        changed = Some((if array { tr_noop!("Remove array stop") } else { tr_noop!("Remove colour stop") }, false));
    }
    // Typed positions and Array drags can pass a neighbour. The pick follows its stop.
    if let Some(picked) = stops.get(sel).cloned() {
        stops.sort_by(|a, b| a.offset().total_cmp(&b.offset()));
        sel = stops.iter().position(|s| *s == picked).unwrap_or(sel);
    }
    if let Some(selected) = selected { *selected = sel; }
    else { ui.data_mut(|d| d.insert_temp(sel_id, sel)); }
    changed
}

fn paint_gradient_strip(p: &egui::Painter, strip: Rect, colour_at: impl Fn(f32) -> egui::Color32) {
    // A checkerboard under the ramp shows its opacity.
    let cell = 4.0;
    let (cols, rows) = ((strip.width() / cell).ceil() as usize, (strip.height() / cell).ceil() as usize);
    p.rect_filled(strip, 0.0, T.header);
    for (cx, cy) in (0..cols).flat_map(|x| (0..rows).map(move |y| (x, y))).filter(|(x, y)| (x + y) % 2 == 0) {
        let r = Rect::from_min_size(pos2(strip.left() + cx as f32 * cell, strip.top() + cy as f32 * cell), vec2(cell, cell)).intersect(strip);
        p.rect_filled(r, 0.0, T.text_faint);
    }
    let mut mesh = egui::Mesh::default();
    let steps = 64u32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let c = colour_at(t);
        let x = strip.left() + t * strip.width();
        mesh.colored_vertex(pos2(x, strip.top()), c);
        mesh.colored_vertex(pos2(x, strip.bottom()), c);
        if i > 0 {
            let k = 2 * i;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k, k + 1);
        }
    }
    p.add(mesh);
    p.rect_stroke(strip, 0.0, Stroke::new(1.0, T.border), StrokeKind::Outside);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Tool;
    use anicraft_engine::geom::Vec2;
    use std::path::PathBuf;

    fn stop(offset: f32, c: u32) -> GradientStop {
        GradientStop { offset, color: Rgba::hex(c) }
    }

    #[test]
    fn stops_are_added_moved_and_removed_in_order() {
        let mut st = vec![stop(0.0, 0x000000), stop(1.0, 0xffffff)];
        // Added where asked, in the ramp's colour there, in order.
        assert_eq!(add_stop(&mut st, 0.25), Some(1));
        assert!((st[1].color.r - 0.25).abs() < 1e-6);
        assert_eq!(widest_gap(&st), 0.625);
        assert_eq!(add_stop(&mut st, f32::NAN), Some(2), "a bad position lands in the middle");
        // A stop moves only between its neighbours.
        move_stop(&mut st, 1, 0.9);
        assert_eq!(st[1].offset, 0.5);
        move_stop(&mut st, 1, -1.0);
        assert_eq!(st[1].offset, 0.0);
        move_stop(&mut st, 9, 0.5);
        // Full at 15 stops; never fewer than 2.
        while add_stop(&mut st, 0.7).is_some() {}
        assert_eq!(st.len(), MAX_GRADIENT_STOPS);
        while remove_stop(&mut st, 0) {}
        assert_eq!(st.len(), 2);
        assert!(!remove_stop(&mut st, 5));
        assert_eq!(hex(Rgba::new(1.0, 0.0, 0.0, 0.5)), "#ff000080");
        assert_eq!(hex(Rgba::hex(0x00ff00)), "#00ff00");
    }

    fn track_frame<S: TrackStop>(ctx: &egui::Context, id: Id, stops: &mut Vec<S>, events: Vec<egui::Event>) {
        let input = egui::RawInput { events, ..Default::default() };
        ctx.run_ui(input, |ui| {
            stop_bar_with(ui, id, stops, None, |_, _, _| false);
        }).textures_delta.clear();
    }

    fn marker(ctx: &egui::Context, id: Id, key: usize) -> egui::Response {
        ctx.read_response(id.with(("fill_stop_marker", key))).unwrap()
    }

    fn track_click(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    fn drag_track<S: TrackStop>(ctx: &egui::Context, id: Id, stops: &mut Vec<S>, from: egui::Pos2, to: egui::Pos2) {
        track_frame(ctx, id, stops, vec![egui::Event::PointerMoved(from)]);
        track_frame(ctx, id, stops, vec![track_click(from, true)]);
        for i in 1..=5 {
            track_frame(ctx, id, stops, vec![egui::Event::PointerMoved(from.lerp(to, i as f32 / 5.0))]);
        }
        track_frame(ctx, id, stops, vec![track_click(to, false)]);
        track_frame(ctx, id, stops, vec![]);
    }

    fn track_key<S: TrackStop>(ctx: &egui::Context, id: Id, stops: &mut Vec<S>, key: Key, modifiers: egui::Modifiers) {
        let press = egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        track_frame(ctx, id, stops, vec![egui::Event::ModifiersChanged(modifiers), press]);
    }

    #[test]
    fn a_property_stop_inherits_before_between_and_after_existing_stops() {
        use anicraft_engine::doc::Easing;
        let mut first = PropertyStop::new(0, 0.2, 0.75);
        first.value.outbound = Some(Easing::QuadOut);
        let mut last = PropertyStop::new(1, 0.8, 0.5);
        last.value.outbound = Some(Easing::BounceOut);
        let mut stops = vec![first.clone(), last.clone()];
        assert_eq!(add_stop(&mut stops, 0.5), Some(1));
        assert_eq!(stops[1].value, ArrayPropertyStop { offset: 0.5, ..first.value.clone() });
        stops[1].value.outbound = Some(Easing::QuadIn);
        assert!(remove_stop(&mut stops, 1));
        assert_eq!(add_stop(&mut stops, 1.0), Some(2));
        assert_eq!(stops[2].value, ArrayPropertyStop { offset: 1.0, ..last.value.clone() });
        assert_eq!(add_stop(&mut stops, 0.0), Some(0));
        assert_eq!(stops[0].value, ArrayPropertyStop { offset: 0.0, ..first.value.clone() });
        assert!(remove_stop(&mut stops, 3) && remove_stop(&mut stops, 0));
        assert_eq!(stops, [first, last]);
    }

    #[test]
    fn property_track_copies_all_values_and_keeps_a_dragged_stop_through_reordering() {
        let ctx = egui::Context::default();
        let id = Id::unique("property_stop_test");
        let mut stops = vec![PropertyStop::new(0, 0.0, 0.75), PropertyStop::new(1, 0.7, 1.0), PropertyStop::new(2, 1.0, 0.03)];
        stops[1].value.scale = [1.7, 0.3];
        stops[1].value.rotation = 700.0;
        track_frame(&ctx, id, &mut stops, vec![]);
        let start = marker(&ctx, id, 0).rect.center();
        let end = marker(&ctx, id, 2).rect.center();
        let at = start.lerp(end, 0.4);
        track_frame(&ctx, id, &mut stops, vec![egui::Event::PointerMoved(at), track_click(at, true)]);
        track_frame(&ctx, id, &mut stops, vec![track_click(at, false)]);
        track_frame(&ctx, id, &mut stops, vec![]);
        let picked = stops[1].clone();
        assert!((picked.value.offset - 0.4).abs() < 1e-4);
        assert_eq!(picked.value, ArrayPropertyStop { offset: picked.value.offset, ..stops[0].value.clone() });
        drag_track(&ctx, id, &mut stops, at, start.lerp(end, 0.85));
        assert_eq!(stops[2].marker, picked.marker);
        assert_eq!(stops[2].value, ArrayPropertyStop { offset: stops[2].value.offset, ..picked.value });
        assert!((stops[2].value.offset - 0.85).abs() < 1e-4);

        let handle = marker(&ctx, id, picked.marker);
        ctx.memory_mut(|m| m.request_focus(handle.id));
        track_frame(&ctx, id, &mut stops, vec![]);
        track_key(&ctx, id, &mut stops, Key::ArrowLeft, egui::Modifiers::SHIFT);
        assert!((stops[2].value.offset - 0.75).abs() < 1e-4);
        track_key(&ctx, id, &mut stops, Key::Delete, egui::Modifiers::NONE);
        assert_eq!(stops.len(), 3);
        assert!(stops.iter().all(|stop| stop.marker != picked.marker));
    }

    #[test]
    fn gradient_track_keeps_colours_and_neighbour_limits_during_drag_and_tear_off() {
        let ctx = egui::Context::default();
        let id = Id::unique("gradient_stop_test");
        let mut stops = vec![stop(0.0, 0xff0000), stop(0.4, 0x00ff00), stop(0.7, 0x0000ff), stop(1.0, 0xffffff)];
        track_frame(&ctx, id, &mut stops, vec![]);
        let at = marker(&ctx, id, 1).rect.center();
        let end = marker(&ctx, id, 3).rect.center();
        drag_track(&ctx, id, &mut stops, at, at.lerp(end, 0.75));
        assert_eq!(stops[1], stop(0.7, 0x00ff00));
        assert_eq!(stops[2], stop(0.7, 0x0000ff));

        // Move one stop left with the keyboard, then tear off the last stop.
        let handle = marker(&ctx, id, 1);
        ctx.memory_mut(|m| m.request_focus(handle.id));
        track_frame(&ctx, id, &mut stops, vec![]);
        track_key(&ctx, id, &mut stops, Key::ArrowLeft, egui::Modifiers::SHIFT);
        assert!((stops[1].offset - 0.6).abs() < 1e-6);
        assert_eq!(stops[1].color, Rgba::hex(0x00ff00));
        track_frame(&ctx, id, &mut stops, vec![]);
        drag_track(&ctx, id, &mut stops, end, end + vec2(0.0, 55.0));
        assert_eq!(stops.len(), 3);
        assert_eq!(stops[2], stop(0.7, 0x0000ff));
    }

    #[test]
    fn delete_on_a_focused_gradient_handle_does_not_delete_the_artwork() {
        use egui::accesskit::{Action, ActionRequest, TreeId};
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::with_context(&ctx, anicraft_engine::Session::default(), Some(PathBuf::from("unused.png")));
        app.run("draw.rect", json!({"x": 10, "y": 10, "w": 40, "h": 30, "fill": "#ff0000", "stroke": null}));
        app.run("paint.set_fill", json!({"type": "linear", "stops": [{"offset": 0, "color": "#ff0000"}, {"offset": 0.5, "color": "#00ff00"}, {"offset": 1, "color": "#0000ff"}]}));
        app.open_fill_swatch = true;
        let mut frame = |events| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 940.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            out.platform_output.accesskit_update.unwrap().nodes
        };
        frame(vec![]);
        let nodes = frame(vec![]);
        let handle = nodes.iter().find(|(_, n)| n.label().is_some_and(|label| label.starts_with("Colour stop 2 of 3"))).unwrap().0;
        frame(vec![egui::Event::AccessKitActionRequest(ActionRequest { action: Action::Focus, target_tree: TreeId::ROOT, target_node: handle, data: None })]);
        assert!(stop_has_focus(&ctx));
        frame(vec![egui::Event::Key { key: Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        let (paint, selected) = current(&app);
        assert!(selected);
        assert_eq!(paint.stops().unwrap().len(), 2);
    }

    /// Draws a few frames of the whole window.
    fn frames(ctx: &egui::Context, app: &mut App) {
        let screen = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 1400.0))), ..Default::default() };
        for _ in 0..2 {
            let mut out = ctx.run_ui(screen.clone(), |ui| app.frame(ui));
            out.textures_delta.clear();
        }
    }

    /// The fill picker drawn alone for `paint`: what it asked for with no input.
    fn picker_once(paint: Option<&Paint>, bitmaps: &[(u64, String)]) -> Option<FillEdit> {
        let ctx = egui::Context::default();
        let mut edit = Some(FillEdit::None(false));
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| edit = fill_picker(ui, Id::unique("test_fill"), paint, bitmaps));
        out.textures_delta.clear();
        edit
    }

    #[test]
    fn the_picker_shows_every_kind_of_fill_without_changing_it() {
        let two = vec![stop(0.0, 0x000000), stop(1.0, 0xffffff)];
        let bitmaps = vec![(7u64, "tile.png".to_string())];
        let mut radial = Paint::radial(Vec2::ZERO, 10.0, two.clone());
        if let Paint::Radial { focal, .. } = &mut radial {
            *focal = 0.4;
        }
        for p in [None, Some(Paint::Solid(Rgba::hex(0x3d8bfd))), Some(Paint::linear(Vec2::ZERO, Vec2::new(10.0, 0.0), two)), Some(radial), Some(Paint::Bitmap { bitmap: anicraft_engine::doc::BitmapId(7), repeat: true, matrix: anicraft_engine::geom::Affine::IDENTITY })] {
            assert_eq!(picker_once(p.as_ref(), &bitmaps), None, "{p:?}");
        }
    }

    #[test]
    fn edits_from_the_picker_reach_the_swatch_and_the_selection() {
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, anicraft_engine::Session::default(), Some(PathBuf::from("unused.png")));
        app.run("draw.rect", json!({"x": 10, "y": 10, "w": 100, "h": 80, "fill": "#ff0000", "stroke": null}));
        app.tool = Tool::Select;
        assert!(matches!(current(&app), (Paint::Solid(_), true)));
        // A type change is one step on the selection and the swatch.
        let steps = app.s.history_len();
        apply_set(&mut app, &FillEdit::Set { label: "Fill", live: false, params: json!({"type": "radial"}) }, true);
        assert_eq!(app.s.history_len(), steps + 1);
        assert!(matches!(current(&app).0, Paint::Radial { .. }) && matches!(app.s.style.fill_paint, Some(Paint::Radial { .. })));
        // A drag of a stop's colour is one step however many frames it takes.
        let steps = app.s.history_len();
        for c in ["#112233", "#445566", "#778899"] {
            let stops = json!([{"offset": 0, "color": c}, {"offset": 1, "color": "#000000"}]);
            apply_set(&mut app, &FillEdit::Set { label: "Colour stop", live: true, params: json!({"stops": stops}) }, true);
        }
        app.s.end_gesture();
        assert_eq!(app.s.history_len(), steps + 1);
        // From the tool options: the swatch only.
        apply_set(&mut app, &FillEdit::Set { label: "Fill", live: false, params: json!({"type": "linear"}) }, false);
        assert!(matches!(current(&app).0, Paint::Radial { .. }), "the selection keeps its fill");
        assert!(matches!(app.s.style.fill_paint, Some(Paint::Linear { .. })));
        // Solid and None are the caller's: apply_set leaves them alone.
        let before = app.s.doc.clone();
        apply_set(&mut app, &FillEdit::Solid([1, 2, 3, 255]), true);
        apply_set(&mut app, &FillEdit::None(true), true);
        assert!(app.s.doc == before);
        // The whole window draws with the Tools square's picker open on the selection's gradient,
        // and on the swatch with a fill tool in hand.
        app.open_fill_swatch = true;
        frames(&ctx, &mut app);
        app.run("edit.deselect", json!({}));
        app.tool = Tool::PaintBucket;
        frames(&ctx, &mut app);
        assert!(!current(&app).1);
        assert_eq!(stops_json(&[stop(0.0, 0xff0000)])[0]["color"], "#ff0000");
        assert_eq!(from_bytes(bytes(Rgba::new(1.0, 0.0, 0.0, 0.5))).a, 128.0 / 255.0);
    }

    /// A solid fill keeps the alpha it was given, a gradient keeps a different alpha on each
    /// stop, and the open fill picker names an Alpha control for either.
    #[test]
    fn a_fill_keeps_its_alpha_and_the_picker_names_it() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::with_context(&ctx, anicraft_engine::Session::default(), Some(PathBuf::from("unused.png")));
        app.run("draw.rect", json!({"x": 10, "y": 10, "w": 40, "h": 30, "fill": "#ff000080", "stroke": null}));
        let Paint::Solid(c) = current(&app).0 else { panic!("a solid fill"); };
        assert!((c.a - 128.0 / 255.0).abs() < 1e-3, "{c:?}");
        app.run("paint.set_fill", json!({"type": "linear", "stops": [{"offset": 0, "color": "#ff0000"}, {"offset": 1, "color": "#0000ff00"}]}));
        let paint = current(&app).0;
        let stops = paint.stops().expect("a gradient");
        assert!((stops[0].color.a - 1.0).abs() < 1e-3 && stops[1].color.a < 1e-3, "{stops:?}");
        app.run("paint.set_fill", json!({"type": "solid", "color": "#3d8bfd80"}));
        app.tool = Tool::Select;
        app.open_fill_swatch = true;
        let mut labels = String::new();
        for _ in 0..4 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 940.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| app.frame(ui));
            out.textures_delta.clear();
            labels = out.platform_output.accesskit_update.map(|u| u.nodes.into_iter().filter_map(|(_, n)| n.label().map(str::to_string)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        }
        assert!(labels.contains("Alpha"), "the fill picker has no Alpha control:\n{labels}");
    }
}
