//! The Array panel retains unfinished settings for recovery after cancellation.

use crate::a11y::Named;
use crate::app::{App, Drag};
use crate::array_gizmo::{Endpoints, Geometry, GizmoDrag, GridGeometry, RADIAL_FIELD_LIMIT};
use crate::colour_panel::PropertyStop;
use crate::dock::{Panel, Place};
use crate::theme::{FORM_LABEL_DIALOG, FORM_LABEL_DOCK, Section, T, Type, form_row, section, text, with_type};
use anicraft_engine::{ArrayEaseMode, ArrayEasing, ArrayPropertyStop, CLOSED_SWEEP_LIMIT, GridPropertyDistribution, GridStagger, GridStaggerAxis, MAX_ARRAY_INSTANCES, RadialDistribution as Radial, Session};
use anicraft_engine::doc::{ColorEffect, Easing, Element, ElementId, ElementKind, Filter, FilterEntry, LayerId, Rgba};
use egui::{Id, Key, Modifiers, Ui};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Linear,
    Grid,
    Radial,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CommitAs {
    #[default]
    Spatial,
    Timeline,
}

struct TimelineConfig {
    position: Option<egui::Pos2>,
    frames: u32,
    hold: bool,
    quit: bool,
    error: Option<anicraft_engine::EngineError>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GridSettings {
    geometry: GridGeometry,
    counts_locked: bool,
    flip_on_reverse: bool,
    property_distribution: GridPropertyDistribution,
    stagger: GridStagger,
}

impl GridSettings {
    fn at(start: [f32; 2]) -> Self {
        Self {
            geometry: GridGeometry { start, counts: [3, 3], spacing: [60.0; 2], spacing_locked: false },
            counts_locked: false,
            flip_on_reverse: true,
            property_distribution: GridPropertyDistribution::default(),
            stagger: GridStagger::default(),
        }
    }

    fn count_limit(self, axis: usize) -> u32 {
        if self.counts_locked {
            MAX_ARRAY_INSTANCES.isqrt()
        } else {
            MAX_ARRAY_INSTANCES / self.geometry.counts[1 - axis]
        }
    }

    fn set_count(&mut self, axis: usize, count: u32) {
        let count = count.clamp(1, self.count_limit(axis));
        let mut counts = self.geometry.counts;
        if self.counts_locked {
            counts = [count; 2];
        } else {
            counts[axis] = count;
        }
        self.geometry.set_counts(counts, axis);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Settings {
    default_origin: [f32; 2],
    count: u32,
    linear: Endpoints,
    radial: Radial,
    grid: GridSettings,
    backwards: bool,
    commit_as: CommitAs,
    create_unique_symbols: bool,
    stops: Vec<PropertyStop>,
    easing: ArrayEasing,
    source_filters: Arc<[FilterEntry]>,
    source_defaults: ArrayPropertyStop,
}

impl Settings {
    fn at([x, y]: [f32; 2]) -> Self {
        let radius = 120.0;
        Self {
            default_origin: [x, y],
            count: 10,
            // Keep the original 36 px default spacing across nine intervals.
            linear: Endpoints { start: [x, y], end: [x + 324.0, y] },
            radial: Radial {
                centre: [x - radius, y], radius, start: 0.0, sweep: CLOSED_SWEEP_LIMIT,
                closed_loop: true, spiral_offset: 0.0, centre_drift: [0.0; 2], rotate: false,
            },
            grid: GridSettings::at([x, y]),
            backwards: false,
            commit_as: CommitAs::Spatial,
            create_unique_symbols: false,
            stops: vec![PropertyStop::new(0, 0.0, 1.0), PropertyStop::new(1, 1.0, 1.0)],
            easing: ArrayEasing::default(),
            source_filters: Arc::default(),
            source_defaults: ArrayPropertyStop::new(0.0, 1.0),
        }
    }

    fn seed_stops(&mut self) {
        for stop in &mut self.stops {
            let offset = stop.value.offset;
            stop.value.clone_from(&self.source_defaults);
            stop.value.offset = offset;
        }
    }

    fn defaults(&self) -> Self {
        let mut settings = Self::at(self.default_origin);
        settings.source_filters = Arc::clone(&self.source_filters);
        settings.source_defaults.clone_from(&self.source_defaults);
        settings.seed_stops();
        settings
    }

    fn params(&self, mode: Mode) -> Value {
        let property_stops: Vec<Value> = self
            .stops
            .iter()
            .map(|s| {
                json!({
                    "offset": s.value.offset, "alpha": s.value.alpha,
                    "color_effect": s.value.color,
                    "filters": s.value.filters,
                    "scale_x": s.value.scale[0], "scale_y": s.value.scale[1], "rotation": s.value.rotation,
                    "outbound_ease": anicraft_engine::params::easing_json(s.value.outbound.as_ref().unwrap_or(&Easing::Linear)),
                })
            })
            .collect();
        let mut params = match mode {
            Mode::Linear => {
                let [dx, dy] = self.linear.spacing(self.count).unwrap_or_default();
                json!({
                    "mode": "linear", "count": self.count,
                    "start_x": self.linear.start[0], "start_y": self.linear.start[1],
                    "dx": dx, "dy": dy,
                })
            }
            Mode::Grid => {
                let GridGeometry { start: [x, y], counts: [columns, rows], spacing: [sx, sy], .. } = self.grid.geometry;
                let axis_params = |axis: GridStaggerAxis| json!({
                    "enabled": axis.enabled, "every": axis.every.get(), "x": axis.offset[0], "y": axis.offset[1],
                });
                json!({
                    "mode": "grid", "rows": rows, "columns": columns,
                    "start_x": x, "start_y": y, "spacing_x": sx, "spacing_y": sy,
                    "flip_on_reverse": self.grid.flip_on_reverse,
                    "property_distribution": self.grid.property_distribution.key(),
                    "stagger": {
                        "rows": axis_params(self.grid.stagger.rows),
                        "columns": axis_params(self.grid.stagger.columns),
                        "accumulate": self.grid.stagger.accumulate,
                    },
                })
            }
            Mode::Radial => json!({
                "mode": "radial", "count": self.count,
                "center_x": self.radial.centre[0], "center_y": self.radial.centre[1],
                "radius": self.radial.radius, "start_angle": self.radial.start,
                "sweep_angle": self.radial.sweep, "closed_loop": self.radial.closed_loop,
                "spiral_offset": self.radial.spiral_offset,
                "drift_x": self.radial.centre_drift[0], "drift_y": self.radial.centre_drift[1],
                "rotate": self.radial.rotate,
            }),
        };
        params["stacking"] = json!(if self.backwards { "front_to_back" } else { "back_to_front" });
        params["source_values"] = json!(true);
        params["property_stops"] = json!(property_stops);
        params["easing"] = json!({
            "mode": self.easing.mode.key(), "preset": anicraft_engine::params::easing_json(&self.easing.preset),
            "distribution": self.easing.distribution, "transform": self.easing.transform,
            "colour_effect": self.easing.colour_effect,
            "filters": self.easing.filters,
        });
        params
    }
}

struct Active {
    mode: Mode,
    history: EditHistory,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditGesture {
    Pointer(egui::ViewportId, Id),
    Text(egui::ViewportId, Id),
    Picker(egui::ViewportId, Id),
}

struct Configuration {
    mode: Mode,
    settings: Settings,
    selected_stop: Option<usize>,
}

#[derive(Default)]
struct EditHistory {
    undo: VecDeque<Configuration>,
    redo: Vec<Configuration>,
    pending: Option<(EditGesture, Configuration)>,
}

impl EditHistory {
    fn record(&mut self, before: Configuration) {
        self.redo.clear();
        self.undo.push_back(before);
        if self.undo.len() > anicraft_engine::history::MAX_STEPS {
            self.undo.pop_front();
        }
    }

    fn settle(&mut self, mode: Mode, settings: &Settings) {
        if let Some((_, before)) = self.pending.take()
            && (before.mode != mode || &before.settings != settings)
        {
            self.record(before);
        }
    }
}

pub struct State {
    active: Option<Active>,
    settings: Option<Settings>,
    draft_source: Option<(LayerId, ElementId)>,
    error: Option<String>,
    reset_pending: bool,
    timeline_config: Option<TimelineConfig>,
    timeline_frames: Option<u32>,
    stop_editor_epoch: u64,
    open_filter_cards: Vec<bool>,
    selected_stop: Option<usize>,
    show_gizmo: bool,
    dim_stage: bool,
    pub(crate) gizmo_drag: Option<GizmoDrag>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            active: None,
            settings: None,
            draft_source: None,
            error: None,
            reset_pending: false,
            timeline_config: None,
            timeline_frames: None,
            stop_editor_epoch: 0,
            open_filter_cards: Vec::new(),
            selected_stop: None,
            show_gizmo: true,
            dim_stage: true,
            gizmo_drag: None,
        }
    }
}

fn source_element(s: &Session) -> Option<Arc<Element>> {
    if s.selection.len() != 1 {
        return None;
    }
    let selected = s.selected_elements();
    let [(layer, element)] = selected.as_slice() else { return None };
    (*layer == s.layer && matches!(element.kind, ElementKind::Symbol { .. })).then(|| Arc::clone(element))
}

#[cfg(test)]
fn source_origin(s: &Session) -> Option<[f32; 2]> {
    source_element(s).map(|element| [element.transform.m[4], element.transform.m[5]])
}

impl State {
    fn mode(&self) -> Option<Mode> {
        self.active.as_ref().map(|active| active.mode)
    }

    fn select_mode(&mut self, s: &mut Session, mode: Option<Mode>) {
        self.check_draft(s);
        if mode == self.mode() {
            return;
        }
        self.gizmo_drag = None;
        let Some(mode) = mode else {
            self.finish(s, false);
            return;
        };
        let before = self.before_edit(None);
        if let Some(active) = &mut self.active {
            active.mode = mode;
            self.record_edit(before, None);
            self.refresh(s);
        } else if let Some(element) = source_element(s) {
            let origin = [element.transform.m[4], element.transform.m[5]];
            let fresh = self.settings.is_none();
            let mut settings = self.settings.clone().unwrap_or_else(|| Settings::at(origin));
            settings.source_defaults = ArrayPropertyStop::from_source(&element);
            let filters = element.kind.filters().unwrap_or_default();
            if settings.source_filters.as_ref() != filters {
                settings.source_filters = Arc::from(filters);
                for stop in &mut settings.stops {
                    stop.value.filters.clone_from(&settings.source_defaults.filters);
                }
            }
            if fresh { settings.seed_stops(); }
            match s.preview_array(&settings.params(mode)) {
                Ok(()) => {
                    self.active = Some(Active { mode, history: EditHistory::default() });
                    self.open_filter_cards = vec![false; settings.source_filters.len()];
                    self.selected_stop = self.selected_stop.filter(|id| settings.stops.iter().any(|stop| stop.marker_id() == *id))
                        .or_else(|| settings.stops.first().map(PropertyStop::marker_id));
                    self.settings = Some(settings);
                    self.draft_source = None;
                    self.error = None;
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }

    fn refresh(&mut self, s: &mut Session) {
        if let (Some(mode), Some(settings)) = (self.mode(), &self.settings) {
            self.error = s.preview_array(&settings.params(mode)).err().map(|e| e.to_string());
        }
    }

    fn timeline_mode(&self) -> Option<Mode> {
        self.mode().filter(|mode| matches!(mode, Mode::Linear | Mode::Radial))
    }

    fn can_commit(&self) -> bool {
        self.settings.as_ref().is_some_and(|settings| {
            settings.commit_as == CommitAs::Spatial || self.timeline_mode().is_some()
        }) && self.error.is_none() && !self.reset_pending
    }

    fn request_commit(&mut self, s: &mut Session, quit: bool) {
        if !self.can_commit() || self.timeline_config.is_some() {
            return;
        }
        if self.settings.as_ref().is_some_and(|settings| settings.commit_as == CommitAs::Timeline) {
            self.timeline_config = Some(TimelineConfig {
                position: None,
                frames: self.timeline_frames.unwrap_or_else(|| anicraft_engine::array_timeline_default_duration(s.doc.stage.fps, s.frame)),
                hold: false,
                quit,
                error: None,
            });
        } else {
            self.finish(s, true);
        }
    }

    pub(crate) fn timeline_modal_open(&self) -> bool {
        self.timeline_config.is_some()
    }

    fn create_timeline(&mut self, s: &mut Session, config: &TimelineConfig) -> anicraft_engine::Result<()> {
        let settings = self.settings.as_ref()
            // i18n-exempt: an engine-style message, translated where it is shown
            .ok_or_else(|| anicraft_engine::EngineError::State("The Array draft is unavailable.".into()))?;
        let mode = self.timeline_mode()
            // i18n-exempt: an engine-style message, translated where it is shown
            .ok_or_else(|| anicraft_engine::EngineError::State("Timeline Animation is available for Linear and Radial Arrays only.".into()))?;
        let mut params = settings.params(mode);
        params["duration"] = json!(config.frames);
        if mode == Mode::Radial {
            params["animation"] = json!("motion");
        } else {
            params["end_x"] = json!(settings.linear.end[0]);
            params["end_y"] = json!(settings.linear.end[1]);
            params["animation"] = json!(if config.hold { "hold" } else { "classic" });
        }
        // The remembered Spatial option belongs only to that output.
        if let Some(fields) = params.as_object_mut() {
            fields.remove("create_unique_symbols");
        }
        s.commit_array_timeline_preview(&params)?;
        self.finish(s, true);
        Ok(())
    }

    fn finish(&mut self, s: &mut Session, commit: bool) {
        if commit && self.error.is_some() {
            return;
        }
        let unique = self.mode().zip(self.settings.as_ref())
            .filter(|(_, settings)| commit && s.array_preview_active() && settings.create_unique_symbols);
        if let Some((mode, settings)) = unique {
            if let Err(error) = s.commit_unique_array_preview(&settings.params(mode)) {
                self.error = Some(error.to_string());
                return;
            }
        } else {
            s.finish_array_preview(commit);
        }
        self.timeline_frames = None;
        let was_active = self.active.take().is_some();
        if commit {
            self.discard_draft();
        } else if was_active {
            // The engine just restored the original, before any preview copies were selected.
            self.draft_source = s.selection.first();
        }
        self.error = None;
        self.timeline_config = None;
        self.reset_pending = false;
        self.gizmo_drag = None;
    }

    pub(crate) fn discard_draft(&mut self) {
        self.settings = None;
        self.timeline_config = None;
        self.draft_source = None;
        self.reset_pending = false;
        self.selected_stop = None;
        self.stop_editor_epoch = self.stop_editor_epoch.wrapping_add(1);
    }

    pub(crate) fn check_draft(&mut self, s: &Session) {
        if let Some(source) = self.draft_source
            && (s.selection.len() != 1 || s.selection.first() != Some(source))
        {
            self.discard_draft();
        }
    }

    fn customized(&self) -> bool {
        self.settings.as_ref().is_some_and(|settings| *settings != settings.defaults())
    }

    fn reset(&mut self, s: &mut Session) {
        let before = self.before_edit(None);
        let defaults = self.settings.as_ref().filter(|_| self.active.is_some()).map(Settings::defaults);
        self.discard_draft();
        self.settings = defaults;
        self.selected_stop = self.settings.as_ref().and_then(|settings| settings.stops.first()).map(PropertyStop::marker_id);
        self.gizmo_drag = None;
        self.record_edit(before, None);
        self.refresh(s);
    }

    // Keep only the settings and the stop pick. The engine still has one open document gesture.
    fn before_edit(&self, gesture: Option<EditGesture>) -> Option<Configuration> {
        let active = self.active.as_ref()?;
        if active.history.pending.as_ref().is_some_and(|(id, _)| Some(*id) == gesture) {
            return None;
        }
        Some(Configuration { mode: active.mode, settings: self.settings.clone()?, selected_stop: self.selected_stop })
    }

    fn differs(&self, before: &Configuration) -> bool {
        self.mode() != Some(before.mode) || self.settings.as_ref() != Some(&before.settings)
    }

    fn record_edit(&mut self, before: Option<Configuration>, gesture: Option<EditGesture>) {
        let Some(before) = before.filter(|before| self.differs(before)) else { return };
        let Some(active) = &mut self.active else { return };
        if active.history.pending.as_ref().is_some_and(|(owner, _)| Some(*owner) == gesture) {
            return;
        }
        // A different edit may arrive before the previous gesture ended.
        active.history.settle(before.mode, &before.settings);
        if let Some(id) = gesture {
            active.history.pending = Some((id, before));
        } else {
            active.history.record(before);
        }
    }

    pub(crate) fn finish_edit(&mut self, ctx: &egui::Context) {
        let (Some(active), Some(settings)) = (&mut self.active, &self.settings) else { return };
        if active.history.pending.as_ref().is_some_and(|(gesture, _)| match *gesture {
            EditGesture::Pointer(viewport, id) => viewport == ctx.viewport_id() && ctx.dragged_id() != Some(id),
            EditGesture::Text(viewport, id) => viewport == ctx.viewport_id() && focused_text(ctx) != Some(id),
            EditGesture::Picker(viewport, id) => viewport == ctx.viewport_id() && !egui::Popup::is_id_open(ctx, id),
        }) {
            active.history.settle(active.mode, settings);
        }
    }

    fn cancel_text_edit(&mut self, s: &mut Session, gesture: EditGesture) {
        let Some(active) = &mut self.active else { return };
        if active.history.pending.as_ref().is_some_and(|(owner, _)| *owner == gesture)
            && let Some((_, before)) = active.history.pending.take()
        {
            self.settings = Some(before.settings);
            self.selected_stop = before.selected_stop;
            self.refresh(s);
        }
    }

    pub(crate) fn can_step(&self, redo: bool) -> bool {
        let Some(active) = &self.active else { return false };
        let changed_edit = active.history.pending.as_ref().is_some_and(|(_, before)| self.differs(before));
        if redo { !changed_edit && !active.history.redo.is_empty() }
        else { changed_edit || !active.history.undo.is_empty() }
    }

    pub(crate) fn step(&mut self, s: &mut Session, redo: bool) -> bool {
        if !self.can_step(redo) { return false; }
        let (Some(active), Some(settings)) = (&mut self.active, &mut self.settings) else { return false };
        let history = &mut active.history;
        history.settle(active.mode, settings);
        let next = if redo { history.redo.pop() } else { history.undo.pop_back() };
        let Some(next) = next else { return false };
        let current = Configuration {
            mode: std::mem::replace(&mut active.mode, next.mode),
            settings: std::mem::replace(settings, next.settings),
            selected_stop: std::mem::replace(&mut self.selected_stop, next.selected_stop),
        };
        if redo { history.undo.push_back(current); } else { history.redo.push(current); }
        self.reset_pending = false;
        self.gizmo_drag = None;
        self.stop_editor_epoch = self.stop_editor_epoch.wrapping_add(1);
        self.refresh(s);
        true
    }

    pub(crate) fn gizmo_geometry(&self) -> Option<Geometry> {
        if !self.show_gizmo { return None; }
        let settings = self.settings.as_ref()?;
        Some(match self.mode()? {
            Mode::Linear => Geometry::Linear(settings.linear),
            Mode::Grid => Geometry::Grid(settings.grid.geometry),
            Mode::Radial => Geometry::Radial(settings.radial),
        })
    }

    #[cfg(test)]
    fn gizmo_endpoints(&self) -> Option<Endpoints> {
        match self.gizmo_geometry()? { Geometry::Linear(ends) => Some(ends), _ => None }
    }

    pub(crate) fn shows_registrations(&self) -> bool {
        self.show_gizmo && self.active.is_some() && self.error.is_none()
    }

    pub(crate) fn gizmo_editable(&self) -> bool {
        !self.reset_pending
    }

    pub(crate) fn set_gizmo_geometry(&mut self, s: &mut Session, geometry: Geometry, gesture: Option<EditGesture>) {
        if self.gizmo_geometry().is_none_or(|old| old == geometry) { return; }
        let before = self.before_edit(gesture);
        if let Some(settings) = &mut self.settings {
            match geometry {
                Geometry::Linear(ends) => settings.linear = ends,
                Geometry::Grid(grid) => settings.grid.geometry = grid,
                Geometry::Radial(radial) => settings.radial = radial,
            }
        }
        self.record_edit(before, gesture);
        self.refresh(s);
    }

    #[cfg(test)]
    fn set_endpoints(&mut self, s: &mut Session, endpoints: Endpoints) {
        self.set_gizmo_geometry(s, Geometry::Linear(endpoints), None);
    }

    pub(crate) fn dim_stage(&self) -> bool {
        self.dim_stage && self.active.is_some() && self.error.is_none()
    }
}

pub(crate) fn blocked(app: &mut App) -> bool {
    if app.s.array_preview_active() {
        app.status_err(tr!("Commit or cancel the Array preview first."));
        true
    } else {
        false
    }
}

pub(crate) fn history_action(id: &str) -> Option<bool> {
    match id {
        "edit.undo" => Some(false),
        "edit.redo" => Some(true),
        _ => None,
    }
}

pub(crate) fn blocked_command(app: &mut App, id: &str) -> bool {
    history_action(id).is_none() && blocked(app)
}

pub(crate) fn history_command(app: &mut App, redo: bool) -> Option<Value> {
    if app.array_panel.step(&mut app.s, redo) {
        let label = tr_noop!("Array edit");
        return Some(if redo { json!({"redone": label}) } else { json!({"undone": label}) });
    }
    app.status_err(if redo { tr!("Nothing to redo in this Array session.") } else { tr!("Nothing to undo in this Array session.") });
    None
}

pub(crate) fn history_shortcut(app: &mut App, ctx: &egui::Context) -> bool {
    if !app.s.array_preview_active() || focused_text(ctx).is_some() || app.modal_open()
        || app.palette.is_some() || egui::Popup::is_any_open(ctx)
    {
        return false;
    }
    let action = ctx.input_mut(|input| {
        // In Array mode this chord is Redo; in the document it keeps its normal binding.
        if input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) {
            return Some("edit.redo");
        }
        app.shortcuts.iter().find_map(|(mods, key, id)| {
            (history_action(id).is_some() && input.consume_key(*mods, *key)).then_some(*id)
        })
    });
    if let Some(action) = action {
        if history_action(action).is_some_and(|redo| app.array_panel.can_step(redo)) {
            ctx.stop_dragging();
        }
        app.invoke(action);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        true
    } else {
        false
    }
}

/// Keep the transaction visible in the editor even when a different dock tab is shown.
pub(crate) fn edit_bar(app: &mut App, ui: &mut Ui) {
    ui.horizontal_centered(|ui| {
        ui.label(text(Type::Label, tr!("Array preview")));
        if (!app.panel_visible(Panel::Array) || app.dock.is_floating(Panel::Array))
            && ui.button(tr!("Show Array")).clicked()
        {
            app.show_panel(Panel::Array);
            if crate::dock_ui::own_windows(app, ui.ctx())
                && let Some(Place::Floating { window, .. }) = app.dock.place(Panel::Array)
                && let Some(float) = app.dock.floating.get(window)
            {
                let viewport = egui::ViewportId::from_hash_of(("dock_float", float.id));
                ui.ctx().send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
            }
        }
    });
}

fn commit_as(state: &mut State, s: &mut Session, ui: &mut Ui) {
    let Some(settings) = state.settings.as_ref().filter(|_| state.active.is_some()) else { return };
    let timeline_supported = state.timeline_mode().is_some();
    let mut output = settings.commit_as;
    ui.label(text(Type::Label, tr!("Commit As")));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (choice, label) in [
            (CommitAs::Spatial, tr!("Spatial Array")),
            (CommitAs::Timeline, tr!("Timeline Animation")),
        ] {
            let response = ui.add_enabled(
                !state.reset_pending && (choice == CommitAs::Spatial || timeline_supported),
                egui::Button::selectable(output == choice, text(Type::Body, label)),
            );
            let response = if choice == CommitAs::Timeline {
                response.on_disabled_hover_text(tr!("Timeline Animation is available for Linear and Radial Arrays only."))
            } else { response };
            if response.clicked() { output = choice; }
        }
    });
    if output != settings.commit_as {
        let before = state.before_edit(None);
        if let Some(settings) = &mut state.settings {
            settings.commit_as = output;
        }
        state.record_edit(before, None);
        if state.error.is_some() { state.refresh(s); }
        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn commit_button(state: &mut State, s: &mut Session, ui: &mut Ui) {
    let commit = ui.add_enabled(state.can_commit(), egui::Button::new(tr!("Commit")));
    let commit = if state.timeline_mode().is_none()
        && state.settings.as_ref().is_some_and(|settings| settings.commit_as == CommitAs::Timeline)
    {
        commit.on_disabled_hover_text(tr!("Timeline Animation is available for Linear and Radial Arrays only."))
    } else {
        commit
    };
    if commit.clicked() {
        state.request_commit(s, false);
        if state.timeline_modal_open() && ui.ctx().viewport_id() != egui::ViewportId::ROOT {
            ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn finish_buttons(state: &mut State, s: &mut Session, ui: &mut Ui) {
    commit_as(state, s, ui);
    if let Some(mut unique) = state.settings.as_ref()
        .filter(|settings| settings.commit_as == CommitAs::Spatial)
        .map(|settings| settings.create_unique_symbols)
    {
        if ui.add_enabled_ui(!state.reset_pending, |ui| {
            crate::theme::checkbox(ui, &mut unique, tr!("Create Unique Symbols"))
        }).inner.changed() {
            let before = state.before_edit(None);
            if let Some(settings) = &mut state.settings {
                settings.create_unique_symbols = unique;
            }
            state.record_edit(before, None);
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    } else {
        ui.label(text(Type::Caption, tr!("Animation settings appear on Commit.")));
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        commit_button(state, s, ui);
        if ui.button(tr!("Cancel")).clicked() {
            state.finish(s, false);
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    });
}

pub(crate) fn show_timeline_modal(app: &mut App, ctx: &egui::Context) {
    let Some(mut config) = app.array_panel.timeline_config.take() else { return };
    let mut cancel = false;
    let mut create = false;
    let id = Id::unique("array_timeline");
    let area = egui::Area::new(id)
        .kind(egui::UiKind::Modal)
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_CENTER)
        .current_pos(config.position.unwrap_or_else(|| ctx.content_rect().center()))
        .constrain_to(ctx.content_rect())
        .movable(false)
        .sense(egui::Sense::hover());
    let mut title_motion = egui::Vec2::ZERO;
    let modal = egui::Modal::new(id).area(area).show(ctx, |ui| {
        ui.set_width(360.0);
        let target = crate::theme::TARGET;
        // A layout scope alone inherits the modal's available height and centers
        // the title in it. Center the widgets only within one title row.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), target),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                cancel = ui.add_sized([target, target], egui::Button::new(text(Type::Label, "×")))
                    .named(tr!("Close")).on_hover_text(tr!("Close")).clicked();
                let title = tr!("Create Timeline Animation");
                title_motion = ui.add_sized(
                    [ui.available_width(), target],
                    egui::Label::new(text(Type::Title, title)).halign(egui::Align::LEFT)
                        .sense(egui::Sense::drag()),
                ).named(title).drag_delta();
            },
        );
        ui.add_space(8.0);
        form_row(ui, FORM_LABEL_DIALOG, tr!("Duration (frames)"), |ui| {
            let max = (anicraft_engine::MAX_FRAME + 1).saturating_sub(app.s.frame).max(2);
            if with_type(ui, Type::Data, |ui| ui.add(
                egui::DragValue::new(&mut config.frames).range(2..=max).max_decimals(0)
            )).named(tr!("Duration (frames)")).changed() {
                config.error = None;
            }
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Duration (seconds)"), |ui| {
            let seconds = f64::from(config.frames) / f64::from(app.s.doc.stage.fps);
            let formatted = format!("{seconds:.2}");
            ui.label(text(Type::Data, &formatted))
                .named(&tr!("{seconds} seconds", seconds = formatted));
        });
        form_row(ui, FORM_LABEL_DIALOG, tr!("Animation Type"), |ui| {
            if app.array_panel.timeline_mode() == Some(Mode::Radial) {
                ui.label(text(Type::Body, tr!("Motion Tween"))).named(tr!("Motion Tween"));
                return;
            }
            let before = config.hold;
            egui::ComboBox::from_id_salt("array_animation_type")
                .selected_text(if config.hold { tr!("Hold Keyframes") } else { tr!("Classic Tween") })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut config.hold, false, tr!("Classic Tween"));
                    ui.selectable_value(&mut config.hold, true, tr!("Hold Keyframes"));
                })
                .response
                .named(tr!("Animation Type"));
            if before != config.hold {
                config.error = None;
            }
        });
        if let Some(error) = &config.error {
            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                ui.label(text(Type::Caption, crate::i18n::t_dyn(&error.to_string())));
            });
        }
        if let Some(anicraft_engine::EngineError::ArrayDuration { suggested: Some(frames), .. }) = config.error.as_ref()
            && ui.button(tr!("Use {frames} frames", frames = frames)).clicked()
        {
            // The engine checked every required key at this exact duration.
            config.frames = *frames;
            config.error = None;
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            cancel |= ui.button(tr!("Cancel")).clicked();
            create = ui.button(tr!("Create Animation")).clicked();
        });
    });
    if title_motion != egui::Vec2::ZERO {
        config.position = Some(modal.response.rect.center() + title_motion);
        ctx.request_repaint();
    }
    app.array_panel.timeline_frames = Some(config.frames);
    if cancel || modal.should_close() {
        app.array_quit_pending = false;
        return;
    }
    if create {
        match app.array_panel.create_timeline(&mut app.s, &config) {
            Ok(()) => {
                let quit = config.quit || std::mem::take(&mut app.array_quit_pending);
                app.array_quit_pending = false;
                if quit {
                    crate::tabs::quit(app);
                }
                ctx.request_repaint();
                return;
            }
            Err(error) => config.error = Some(error),
        }
    }
    app.array_panel.timeline_config = Some(config);
}

pub(crate) fn can_apply_on_quit(app: &App) -> bool {
    app.array_panel.can_commit()
}

pub(crate) fn finish_for_quit(app: &mut App, apply: bool) {
    if apply {
        app.array_panel.request_commit(&mut app.s, true);
    } else {
        app.array_panel.finish(&mut app.s, false);
    }
    app.array_quit_pending = false;
    if !app.s.array_preview_active() {
        crate::tabs::quit(app);
    } else if apply && !app.array_panel.timeline_modal_open() {
        app.show_panel(Panel::Array);
    }
}

fn text_focus_id(ctx: &egui::Context) -> Id {
    Id::unique(("array_text_focus", ctx.viewport_id()))
}

pub(crate) fn remember_text_focus(ctx: &egui::Context) {
    let focus = focused_text(ctx);
    let key = text_focus_id(ctx);
    ctx.data_mut(|d| d.insert_temp(key, focus));
}

fn focused_text(ctx: &egui::Context) -> Option<Id> {
    ctx.memory(|m| m.focused()).filter(|_| ctx.text_edit_focused())
}

pub(crate) fn finish_frame(app: &mut App, ctx: &egui::Context) {
    app.array_panel.finish_edit(ctx);
    app.array_panel.check_draft(&app.s);
    if !app.panel_visible(Panel::Array) {
        app.array_panel.reset_pending = false;
    }
    remember_text_focus(ctx);
}

/// A live preview with property stops and surrounding artwork for screenshot checks.
pub(crate) fn screenshot(app: &mut App, view: &str) {
    let stagger_sample = view == "array-grid-stagger";
    let unique_sample = view == "array-grid-unique";
    let grid_sample = stagger_sample || unique_sample || matches!(view, "array-grid" | "array-grid-flipped");
    let segment_sample = matches!(view, "array-easing-segment" | "array-easing-segment-middle" | "array-easing-segment-end");
    let colour_sample = matches!(view, "array-colour-tint" | "array-colour-brightness" | "array-colour-advanced");
    let blur_sample = matches!(view, "array-blur" | "array-blur-spiral" | "array-blur-circle");
    let shadow_glow_sample = matches!(view, "array-shadow-glow" | "array-glow-shadow");
    let bevel_sample = view == "array-bevel";
    let gradient_glow_sample = view == "array-gradient-glow";
    let gradient_bevel_sample = view == "array-gradient-bevel";
    let adjust_color_sample = view == "array-adjust-color";
    let transform_sample = blur_sample || colour_sample || segment_sample || matches!(view, "array-transform" | "array-transform-radial" | "array-easing-global");
    let open_radial = view.strip_prefix("array-radial-open:").and_then(|setup| {
        let mut values = setup.split(',');
        let sweep = values.next()?.parse::<f32>().ok()?;
        let offset = values.next()?.parse::<f32>().ok()?;
        let mut drift = [0.0; 2];
        if let Some(x) = values.next() {
            drift = [x.parse::<f32>().ok()?, values.next()?.parse::<f32>().ok()?];
        }
        Some((sweep, offset, drift))
    });
    app.perform("file.new", json!({"width": 640, "height": 400}));
    app.run("draw.rect", json!({"x": 72, "y": 72, "w": 125, "h": 60, "fill": "#376b93", "stroke": null}));
    app.run("draw.ellipse", json!({"x": 494, "y": 275, "w": 82, "h": 68, "fill": "#376b93", "stroke": null}));
    let diameter = if open_radial.is_some() { 20 } else { 56 };
    if grid_sample {
        app.run("draw.path", json!({
            "d": "M 150 136 L 166 136 L 166 149 L 187 149 L 187 160 L 150 160 Z",
            "fill": "#ce523f", "stroke": null,
        }));
    } else if transform_sample {
        app.run("draw.rect", json!({"x": 92, "y": 172, "w": 34, "h": 15, "fill": "#ce523f", "stroke": null}));
    } else {
        app.run("draw.ellipse", json!({"x": 135, "y": 170, "w": diameter, "h": diameter, "fill": "#ce523f", "stroke": null}));
    }
    app.run("modify.convert_to_symbol", json!({"name": "Array sample"}));
    if bevel_sample {
        app.run("symbol.set_kind", json!({"kind": "movie_clip"}));
        app.run("element.set_filters", json!({"filters": [{"type": "bevel", "blur_x": 8, "blur_y": 8,
            "angle": 350, "distance": 8, "quality": 3, "side": "full", "strength": 1.8,
            "shadow_color": "#391946", "highlight_color": "#fff0b4"}]}));
    } else if gradient_glow_sample {
        app.run("symbol.set_kind", json!({"kind": "movie_clip"}));
        app.run("element.set_filters", json!({"filters": [{"type": "gradient_glow", "blur_x": 19, "blur_y": 15,
            "angle": 25, "distance": 9, "quality": 3, "side": "outer", "strength": 1.7,
            "stops": [{"offset": 0, "color": "#ffffff00"}, {"offset": 0.2, "color": "#30136850"},
                {"offset": 0.65, "color": "#6451edcc"}, {"offset": 1, "color": "#2ad9c9ff"}]}]}));
    } else if gradient_bevel_sample {
        app.run("symbol.set_kind", json!({"kind": "movie_clip"}));
        app.run("element.set_filters", json!({"filters": [{"type": "gradient_bevel", "blur_x": 9, "blur_y": 8,
            "angle": 40, "distance": 12, "quality": 3, "side": "full", "strength": 2.2,
            "stops": [{"offset": 0, "color": "#fff1abff"}, {"offset": 0.25, "color": "#f6ae4cbf"},
                {"offset": 0.5, "color": "#00000000"}, {"offset": 0.75, "color": "#872bb1ae"},
                {"offset": 1, "color": "#431775ff"}]}]}));
    } else if adjust_color_sample {
        app.run("symbol.set_kind", json!({"kind": "movie_clip"}));
        app.run("element.set_filters", json!({"filters": [{"type": "adjust_color", "brightness": 6,
            "contrast": 18, "saturation": 26, "hue": -125}]}));
    } else if blur_sample || shadow_glow_sample {
        app.run("symbol.set_kind", json!({"kind": "movie_clip"}));
        let blur = json!({"type": "blur", "blur_x": 1.5, "blur_y": 0, "quality": 2});
        let mut filters = vec![blur];
        if shadow_glow_sample {
            filters.splice(..0, [
                json!({"type": "drop_shadow", "blur_x": 8, "blur_y": 8, "distance": 22, "angle": 350, "color": "#46276e", "quality": 2}),
                json!({"type": "glow", "blur_x": 18, "blur_y": 18, "color": "#03bfba", "strength": 1.6, "quality": 2}),
            ]);
            if view == "array-glow-shadow" { filters.swap(0, 1); }
        }
        app.run("element.set_filters", json!({"filters": filters}));
    }
    if matches!(view, "array-polish" | "array-polish-off" | "array-radial")
        && let Some((_, source)) = app.s.selection.first()
    {
        app.run("draw.rect", json!({"x": 253, "y": 160, "w": 125, "h": 112, "fill": "#376b93", "stroke": null}));
        app.s.select_only([source]);
    }
    app.show_panel(Panel::Array);
    if app.shot.is_some() {
        for group in &mut app.dock.right.groups {
            group.folded = !group.tabs.contains(&Panel::Array);
        }
    }
    app.tool = crate::app::Tool::Select;
    if shadow_glow_sample || bevel_sample {
        // Keep the mixed-filter capture at a fixed scale and draw the artwork before capture.
        app.view.fit = false;
        app.view.zoom = 1.0;
        app.prefs.render_mode = crate::prefs::RenderMode::Immediate;
    }
    if gradient_glow_sample || gradient_bevel_sample || adjust_color_sample {
        app.dock.right.size = Some(360.0);
        app.prefs.render_mode = crate::prefs::RenderMode::Immediate;
    }
    let mode = if grid_sample {
        app.dock.right.size = Some(345.0);
        Mode::Grid
    } else if segment_sample || matches!(view, "array-radial" | "array-transform-radial" | "array-colour-advanced" | "array-blur-spiral" | "array-blur-circle") || open_radial.is_some() {
        Mode::Radial
    } else {
        Mode::Linear
    };
    app.array_panel.select_mode(&mut app.s, Some(mode));
    if view != "array-blur-circle" { app.array_panel.open_filter_cards.fill(true); }
    app.array_panel.show_gizmo = !matches!(view, "array-hidden" | "array-polish-off");
    app.array_panel.dim_stage = view != "array-polish-off";
    if let Some(settings) = &mut app.array_panel.settings {
        settings.linear.end[1] += 50.0;
        if mode == Mode::Radial {
            settings.radial.centre = [320.0, 200.0];
        }
        if let Some((sweep, offset, drift)) = open_radial {
            settings.radial.closed_loop = false;
            settings.radial.sweep = sweep;
            settings.radial.spiral_offset = offset;
            settings.radial.centre_drift = drift;
            if offset != 0.0 {
                settings.count = 24;
                settings.radial.radius = if offset > 0.0 { 38.0 } else { 140.0 };
            } else if drift != [0.0; 2] {
                settings.count = 36;
                settings.radial.radius = 65.0;
            }
        } else if grid_sample {
            settings.create_unique_symbols = unique_sample;
            if stagger_sample {
                settings.grid.stagger.rows.enabled = true;
                settings.grid.stagger.columns.enabled = true;
                settings.grid.stagger.columns.offset = [0.0, -30.0];
            }
            let grid = &mut settings.grid.geometry;
            grid.counts = [4, 3];
            if view == "array-grid-flipped" {
                grid.start = [456.0, 290.0];
                grid.spacing = [-84.0, -70.0];
            } else {
                grid.start = [160.0, 132.0];
                grid.spacing = [84.0, 70.0];
            }
        } else {
            settings.stops =
                vec![PropertyStop::new(0, 0.0, 0.75), PropertyStop::new(2, 0.4, 1.0), PropertyStop::new(1, 1.0, 0.03)];
        }
        if transform_sample {
            settings.count = if mode == Mode::Radial { 17 } else { 12 };
            settings.linear.end[0] += 140.0;
            for (stop, (scale, rotation, alpha)) in settings.stops.iter_mut().zip([
                ([0.65, 1.15], 20.0, 0.75), ([1.05, 0.7], 320.0, 1.0), ([1.55, 1.0], 720.0, 0.45),
            ]) {
                stop.value.scale = scale;
                stop.value.rotation = rotation;
                stop.value.alpha = alpha;
            }
            if mode == Mode::Radial && view != "array-blur-circle" {
                settings.radial.centre = [270.0, 185.0];
                settings.radial.closed_loop = false;
                settings.radial.sweep = 630.0;
                settings.radial.radius = 58.0;
                settings.radial.spiral_offset = 26.0;
                settings.radial.centre_drift = [40.0, 12.0];
                settings.radial.rotate = true;
            }
        }
        match view {
            "array-blur" | "array-blur-spiral" | "array-blur-circle" => {
                for (stop, [blur_x, blur_y]) in settings.stops.iter_mut().zip([[1.5, 0.0], [22.0, 3.0], [4.0, 26.0]]) {
                    stop.value.alpha = 1.0;
                    stop.value.filters = vec![Filter::Blur { blur_x, blur_y, quality: 2 }];
                }
                app.array_panel.selected_stop = settings.stops.get(1).map(PropertyStop::marker_id);
            }
            "array-bevel" => {
                settings.count = 5;
                settings.linear.end[0] += 110.0;
                for (stop, (direction, shadow, highlight)) in settings.stops.iter_mut().zip([
                    (350.0, 0x391946, 0xfff0b4), (25.0, 0x2c194d, 0xbef5f3), (85.0, 0x401641, 0xf5c5ff),
                ]) {
                    stop.value.alpha = 1.0;
                    stop.value.filters.clone_from(&settings.source_defaults.filters);
                    if let Some(Filter::Bevel { angle, shadow_color, highlight_color, .. }) = stop.value.filters.first_mut() {
                        *angle = direction;
                        *shadow_color = Rgba::hex(shadow);
                        *highlight_color = Rgba::hex(highlight);
                    }
                }
                app.array_panel.selected_stop = settings.stops.get(1).map(PropertyStop::marker_id);
            }
            "array-gradient-glow" => {
                settings.count = 5;
                settings.linear.end[0] += 75.0;
                for (stop, (offset, inner, middle, outer)) in settings.stops.iter_mut().zip([
                    (0.20, 0x301368, 0x6451ed, 0x2ad9c9), (0.36, 0x1c4382, 0x3678df, 0xcff6b3), (0.54, 0x95431c, 0xdc862f, 0xffdc7d),
                ]) {
                    stop.value.alpha = 1.0;
                    stop.value.filters.clone_from(&settings.source_defaults.filters);
                    if let Some(Filter::GradientGlow { stops, .. }) = stop.value.filters.first_mut() {
                        for (entry, (rgb, opacity)) in stops.iter_mut().skip(1).zip([(inner, 0.35), (middle, 0.8), (outer, 1.0)]) {
                            entry.color = Rgba::hex(rgb).with_alpha(opacity);
                        }
                        if let Some(entry) = stops.get_mut(1) { entry.offset = offset; }
                    }
                }
                app.array_panel.selected_stop = settings.stops.get(1).map(PropertyStop::marker_id);
            }
            "array-gradient-bevel" => {
                settings.count = 5;
                settings.linear.end[0] += 75.0;
                for (stop, (left, right, highlight, shadow)) in settings.stops.iter_mut().zip([
                    (0.15, 0.78, 0xffedaa, 0x681e8b), (0.29, 0.72, 0xcaff9a, 0x204da2), (0.39, 0.86, 0xc3fcff, 0x125d70),
                ]) {
                    stop.value.alpha = 1.0;
                    stop.value.filters.clone_from(&settings.source_defaults.filters);
                    if let Some(Filter::GradientBevel { stops, .. }) = stop.value.filters.first_mut()
                        && let [start, before, _, after, end] = stops.as_mut_slice()
                    {
                        start.color = Rgba::hex(highlight);
                        before.offset = left;
                        before.color = start.color.with_alpha(0.8);
                        after.offset = right;
                        after.color = Rgba::hex(shadow).with_alpha(0.72);
                        end.color = Rgba::hex(shadow);
                    }
                }
                app.array_panel.selected_stop = settings.stops.get(1).map(PropertyStop::marker_id);
            }
            "array-adjust-color" => {
                settings.count = 5;
                settings.linear.end[0] += 75.0;
                for (stop, (brightness, contrast, saturation, hue)) in settings.stops.iter_mut().zip([
                    (6.0, 18.0, 26.0, -125.0), (15.0, 4.0, 5.0, -18.0), (-8.0, 36.0, 45.0, 126.0),
                ]) {
                    stop.value.alpha = 1.0;
                    stop.value.filters = vec![Filter::AdjustColor { brightness, contrast, saturation, hue }];
                }
                app.array_panel.selected_stop = settings.stops.get(1).map(PropertyStop::marker_id);
            }
            "array-shadow-glow" | "array-glow-shadow" => {
                settings.count = 6;
                settings.linear.end[0] += 110.0;
                let shadows = [(350.0, 20.0, 0x46276e), (45.0, 30.0, 0x985410), (115.0, 40.0, 0x1a42b8)];
                let glows = [(18.0, 18.0, 1.6, 0x03bfba), (26.0, 14.0, 2.2, 0xffb13f), (13.0, 25.0, 1.4, 0xbe4cdf)];
                for ((stop, shadow), glow) in settings.stops.iter_mut().zip(shadows).zip(glows) {
                    stop.value.alpha = 1.0;
                    stop.value.filters.clone_from(&settings.source_defaults.filters);
                    for filter in &mut stop.value.filters {
                        match filter {
                            Filter::DropShadow { angle, distance, color, .. } => {
                                *angle = shadow.0;
                                *distance = shadow.1;
                                *color = Rgba::hex(shadow.2);
                            }
                            Filter::Glow { blur_x, blur_y, strength, color, .. } => {
                                *blur_x = glow.0;
                                *blur_y = glow.1;
                                *strength = glow.2;
                                *color = Rgba::hex(glow.3);
                            }
                            _ => {}
                        }
                    }
                }
                app.array_panel.selected_stop = settings.stops.get(1).map(PropertyStop::marker_id);
            }
            "array-colour-brightness" => {
                for (stop, value) in settings.stops.iter_mut().zip([-0.8, 0.0, 0.75]) {
                    stop.value.color = ColorEffect::Brightness(value);
                }
            }
            "array-colour-tint" => {
                for (stop, (rgb, amount)) in settings.stops.iter_mut().zip([(0x2675ed, 0.9), (0xcf23c8, 0.75), (0x08c576, 0.9)]) {
                    stop.value.color = ColorEffect::Tint { color: Rgba::hex(rgb), amount };
                }
            }
            "array-colour-advanced" => {
                let transform = anicraft_engine::doc::ColorTransform {
                    mul: [0.65, 0.85, 0.45, 1.0], add: [20.0, -15.0, 145.0, 24.0].map(|v| v / 255.0),
                };
                for (stop, color) in settings.stops.iter_mut().zip([
                    ColorEffect::Advanced(transform), ColorEffect::Brightness(0.3),
                    ColorEffect::Tint { color: Rgba::hex(0x08a675), amount: 0.8 },
                ]) {
                    stop.value.color = color;
                }
            }
            "array-easing-global" => settings.easing.preset = Easing::BackOut,
            _ if segment_sample => {
                settings.easing.mode = ArrayEaseMode::PerSegment;
                settings.easing.colour_effect = false;
                for (stop, preset) in settings.stops.iter_mut().zip([Easing::QuadOut, Easing::SineIn]) {
                    stop.value.outbound = Some(preset);
                }
                let selected = match view {
                    "array-easing-segment-middle" => 1,
                    "array-easing-segment-end" => 2,
                    _ => 0,
                };
                app.array_panel.selected_stop = settings.stops.get(selected).map(PropertyStop::marker_id);
            }
            _ => {}
        }
    }
    app.array_panel.refresh(&mut app.s);
}

pub(crate) fn text_had_escape(ctx: &egui::Context) -> bool {
    current_or_previous_text(ctx).is_some()
}

fn current_or_previous_text(ctx: &egui::Context) -> Option<Id> {
    // egui surrenders ordinary TextEdit focus before the frame on Escape. DragValue still
    // needs to see the key to discard the typed value rather than applying it on blur.
    let key = text_focus_id(ctx);
    let prior = ctx.data(|d| d.get_temp::<Option<Id>>(key)).flatten();
    focused_text(ctx).or_else(|| prior.filter(|id| ctx.memory(|m| m.had_focus_last_frame(*id))))
}

pub(crate) fn escape(app: &mut App, ctx: &egui::Context) {
    if !(app.s.array_preview_active() || app.array_panel.reset_pending)
        || app.modal_open()
        || app.palette.is_some()
        || app.dock_view.drag.is_some()
        || egui::Popup::is_any_open(ctx)
    {
        return;
    }
    if let Some(id) = current_or_previous_text(ctx) {
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            app.array_panel.cancel_text_edit(&mut app.s, EditGesture::Text(ctx.viewport_id(), id));
        }
        return;
    }
    if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        if app.array_panel.reset_pending {
            app.array_panel.reset_pending = false;
        } else {
            app.array_panel.finish(&mut app.s, false);
        }
    }
}

/// Called wherever the Array dock tab is drawn.
pub fn panel(app: &mut App, ui: &mut Ui) {
    app.array_panel.check_draft(&app.s);
    let eligible = app.s.array_preview_active() || source_element(&app.s).is_some();
    let busy = app.playing
        || app.drag != Drag::None
        || app.text_edit.is_some()
        || app.pen.is_some()
        || app.modal_open()
        || app.file_dialog_open();
    let state = &mut app.array_panel;
    let prior = state.mode();
    let mut mode = prior;
    form_row(ui, FORM_LABEL_DOCK, tr!("Array type"), |ui| {
        egui::ComboBox::from_id_salt("array_type")
            .selected_text(match mode {
                None => tr!("None"),
                Some(Mode::Linear) => tr!("Linear"),
                Some(Mode::Grid) => tr!("Grid"),
                Some(Mode::Radial) => tr!("Radial"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, None, tr!("None"));
                ui.add_enabled_ui(eligible, |ui| {
                    ui.selectable_value(&mut mode, Some(Mode::Linear), tr!("Linear"));
                    ui.selectable_value(&mut mode, Some(Mode::Grid), tr!("Grid"));
                    ui.selectable_value(&mut mode, Some(Mode::Radial), tr!("Radial"));
                });
                ui.add_enabled(false, egui::Button::selectable(false, tr!("Along Path")));
            })
            .response
            .named(tr!("Array type"));
    });
    if mode != prior {
        if prior.is_none() && busy {
            state.error = Some(tr!("Finish the current edit before starting an array.").to_string());
        } else {
            state.select_mode(&mut app.s, mode);
            if state.active.is_some() {
                // Preview controls have their own input, independent of the stage and timeline.
                app.path_pick = None;
            }
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    }
    if !eligible {
        ui.label(text(Type::Caption, tr!("Select one symbol instance on the active layer to make an array.")));
    }

    if let Some(mode) = state.mode() {
        if state.mode().is_some() && view_options(ui, state) {
            app.stage_cache.shown_state = None;
        }
        if state.mode().is_some() {
            let ctx = ui.ctx();
            let viewport = ctx.viewport_id();
            let mut gesture = ctx.dragged_id().or_else(|| ctx.drag_stopped_id()).map(|id| EditGesture::Pointer(viewport, id))
                .or_else(|| focused_text(ctx).map(|id| EditGesture::Text(viewport, id)));
            let before = state.before_edit(gesture);
            let changed = ui.add_enabled_ui(!state.reset_pending, |ui| {
                settings_ui(ui, state, mode, &mut app.scale_lock, &mut gesture)
            }).inner;
            if changed {
                state.record_edit(before, gesture);
                state.refresh(&mut app.s);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        }
    }
    if let Some(error) = &state.error {
        ui.label(text(Type::Caption, crate::i18n::t_dyn(error)));
    }

    ui.add_space(8.0);
    if state.reset_pending {
        ui.label(text(Type::Body, tr!("Reset the remembered Array settings?")));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr!("Reset Settings")).clicked() {
                state.reset(&mut app.s);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            if ui.button(tr!("Keep Settings")).clicked() {
                state.reset_pending = false;
            }
        });
    } else if ui.add_enabled(state.settings.is_some(), egui::Button::new(tr!("Reset Settings to Defaults"))).clicked() {
        if state.customized() {
            state.reset_pending = true;
        } else {
            state.reset(&mut app.s);
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
    }
    if state.active.is_some() {
        ui.separator();
        finish_buttons(state, &mut app.s, ui);
    }
}

fn view_options(ui: &mut Ui, state: &mut State) -> bool {
    ui.horizontal_wrapped(|ui| {
        let dim = crate::theme::checkbox(ui, &mut state.dim_stage, tr!("Dim Stage")).changed();
        let tip = if state.show_gizmo {
            tr!("Hide Array Handles")
        } else {
            tr!("Show Array Handles")
        };
        let target = crate::theme::TARGET;
        let (rect, toggle) = ui.allocate_exact_size(egui::vec2(target + 4.0, target), egui::Sense::click());
        let hot = toggle.hovered() || toggle.has_focus();
        let (fill, icon) = if state.show_gizmo { (T.active, T.on_active) }
            else if hot { (T.hover, T.icon_hot) } else { (T.header, T.icon) };
        ui.painter().rect(rect, T.rc(), fill, egui::Stroke::new(1.0, T.border), egui::StrokeKind::Inside);
        crate::icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(crate::theme::TOOL_ICON)),
            crate::icons::Icon::ArrayHandles, icon);
        crate::a11y::control(ui, &toggle, egui::Role::Button, tip, Some(state.show_gizmo));
        crate::a11y::focus_ring(ui, &toggle);
        let toggle = toggle.on_hover_text(tip);
        if toggle.clicked() {
            state.show_gizmo = !state.show_gizmo;
            state.gizmo_drag = None;
        }
        if dim || toggle.clicked() {
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
        dim
    }).inner
}

fn settings_ui(ui: &mut Ui, state: &mut State, mode: Mode, scale_lock: &mut bool, gesture: &mut Option<EditGesture>) -> bool {
    let State { settings: Some(settings), stop_editor_epoch, selected_stop, open_filter_cards, .. } = state else { return false };
    let grid = mode == Mode::Grid;
    let mut changed = grid && distribution_fields(ui, settings, mode);
    if grid {
        ui.add_space(4.0);
        changed |= stagger_fields(ui, &mut settings.grid.stagger);
        ui.add_space(4.0);
    }
    let easing_open = settings.easing != ArrayEasing::default();
    changed |= crate::theme::section_with_default(ui, "array_easing", Section::Rule, tr!("Easing"), easing_open, |ui| {
        let index = settings.stops.iter().position(|stop| Some(stop.marker_id()) == *selected_stop).unwrap_or(0);
        let has_successor = index + 1 < settings.stops.len();
        let has_filters = settings.stops.first().is_some_and(|stop| !stop.value.filters.is_empty());
        easing_fields(ui, &mut settings.easing, settings.stops.get_mut(index), has_successor, has_filters, mode != Mode::Grid)
    }).unwrap_or(false);
    ui.add_space(4.0);
    section(ui, "array_stops", Section::Rule, tr!("Property stops"), None, |ui| {
        let id = ui.make_persistent_id(("array_stop_bar", *stop_editor_epoch));
        let before = *selected_stop;
        changed |= crate::colour_panel::array_stop_bar(ui, id, &mut settings.stops, selected_stop, |ui, stop| {
            stop_properties(ui, stop, &settings.source_filters, open_filter_cards, scale_lock, gesture)
        });
        if *selected_stop != before { ui.ctx().request_repaint(); }
    });
    if !grid {
        ui.add_space(4.0);
        changed |= distribution_fields(ui, settings, mode);
    }
    changed
}

fn distribution_fields(ui: &mut Ui, settings: &mut Settings, mode: Mode) -> bool {
    let mut changed = false;
    section(ui, "array_distribution", Section::Rule, tr!("Distribution"), None, |ui| {
        if mode != Mode::Grid {
            ui.horizontal_wrapped(|ui| {
                ui.label(text(Type::Label, tr!("Instances")));
                changed |= with_type(ui, Type::Data, |ui| {
                    ui.add(egui::DragValue::new(&mut settings.count).range(1..=MAX_ARRAY_INSTANCES))
                }).named(tr!("Instances")).changed();
            });
        }
        match mode {
            Mode::Linear => {
                changed |= coordinates(ui, tr!("Start"), [tr!("Start X"), tr!("Start Y")], &mut settings.linear.start, " px");
                changed |= coordinates(ui, tr!("End"), [tr!("End X"), tr!("End Y")], &mut settings.linear.end, " px");
                changed |= linear_dimensions(ui, &mut settings.linear, settings.count);
            }
            Mode::Grid => changed |= grid_fields(ui, &mut settings.grid),
            Mode::Radial => changed |= radial_fields(ui, &mut settings.radial),
        }
        changed |= crate::theme::checkbox(ui, &mut settings.backwards, tr!("Stack first copy in front")).changed();
    });
    changed
}

fn grid_fields(ui: &mut Ui, grid: &mut GridSettings) -> bool {
    use crate::{icons::Icon, timeline::icon_button};

    let before = *grid;
    for (axis, label) in [(1, tr!("Rows")), (0, tr!("Columns"))] {
        form_row(ui, FORM_LABEL_DOCK, label, |ui| {
            ui.horizontal(|ui| {
                let mut count = grid.geometry.counts[axis];
                if with_type(ui, Type::Data, |ui| {
                    ui.add(egui::DragValue::new(&mut count).range(1..=grid.count_limit(axis))
                        .clamp_existing_to_range(false))
                }).named(label).changed() {
                    grid.set_count(axis, count);
                }
                if axis == 0 && grid_lock(ui, tr!("Lock row and column counts"), &mut grid.counts_locked)
                    && grid.counts_locked
                {
                    grid.set_count(axis, grid.geometry.counts[axis]);
                }
            });
        });
    }
    for (axis, label) in [(1, tr!("Row Spacing (Y)")), (0, tr!("Column Spacing (X)"))] {
        form_row(ui, FORM_LABEL_DOCK, label, |ui| {
            ui.horizontal(|ui| {
                let mut spacing = grid.geometry.spacing[axis];
                if dimension_value(ui, label, &mut spacing, -1_000_000.0..=1_000_000.0, " px", true)
                    && spacing.is_finite()
                {
                    grid.geometry.set_spacing(axis, spacing);
                }
                if axis == 0 && grid_lock(ui, tr!("Lock spacing"), &mut grid.geometry.spacing_locked)
                    && grid.geometry.spacing_locked
                {
                    let spacing = grid.geometry.spacing[axis];
                    grid.geometry.set_spacing(axis, spacing);
                }
            });
        });
    }
    let mut start = grid.geometry.start;
    if coordinates(ui, tr!("Start"), [tr!("Start X"), tr!("Start Y")], &mut start, " px")
        && start.into_iter().all(f32::is_finite)
    {
        grid.geometry.start = start;
    }
    let label = tr!("Flip on Reverse");
    form_row(ui, FORM_LABEL_DOCK, label, |ui| {
        if icon_button(ui, label, Some(grid.flip_on_reverse), Icon::Flip).clicked() {
            grid.flip_on_reverse = !grid.flip_on_reverse;
        }
    });
    form_row(ui, FORM_LABEL_DOCK, tr!("Property Distribution"), |ui| {
        ui.horizontal_wrapped(|ui| {
            for (mode, label, icon) in [
                (GridPropertyDistribution::X, tr!("X"), Icon::DistributeH),
                (GridPropertyDistribution::Both, tr!("X + Y"), Icon::Rect),
                (GridPropertyDistribution::Y, tr!("Y"), Icon::DistributeV),
            ] {
                ui.vertical(|ui| {
                    if icon_button(ui, label, Some(grid.property_distribution == mode), icon).clicked() {
                        grid.property_distribution = mode;
                    }
                    ui.label(text(Type::Caption, label));
                });
            }
        });
    });
    *grid != before
}

fn stagger_fields(ui: &mut Ui, stagger: &mut GridStagger) -> bool {
    let before = *stagger;
    crate::theme::section_with_default(ui, "array_stagger", Section::Rule, tr!("Stagger"),
        stagger.rows.enabled || stagger.columns.enabled, |ui| {
            ui.push_id("rows", |ui| stagger_axis_fields(ui, &mut stagger.rows, tr!("Offset Rows"),
                [tr!("Row offset Every N"), tr!("Row X Offset"), tr!("Row Y Offset")]));
            ui.push_id("columns", |ui| stagger_axis_fields(ui, &mut stagger.columns, tr!("Offset Columns"),
                [tr!("Column offset Every N"), tr!("Column X Offset"), tr!("Column Y Offset")]));
            crate::theme::checkbox(ui, &mut stagger.accumulate, tr!("Accumulate"));
        });
    *stagger != before
}

fn stagger_axis_fields(ui: &mut Ui, axis: &mut GridStaggerAxis, label: &str, [every, x_name, y_name]: [&str; 3]) {
    crate::theme::checkbox(ui, &mut axis.enabled, label);
    form_row(ui, FORM_LABEL_DOCK, tr!("Every N"), |ui| {
        let mut value = axis.every.get();
        with_type(ui, Type::Data, |ui| {
            ui.add(egui::DragValue::new(&mut value).range(1..=u32::MAX))
        }).named(every);
        if let Some(value) = NonZeroU32::new(value) {
            axis.every = value;
        }
    });
    for ((label, name), offset) in [(tr!("X Offset"), x_name), (tr!("Y Offset"), y_name)].into_iter().zip(&mut axis.offset)
    {
        form_row(ui, FORM_LABEL_DOCK, label, |ui| {
            let mut value = *offset;
            if dimension_value(ui, name, &mut value, -1_000_000.0..=1_000_000.0, " px", true) && value.is_finite() {
                *offset = value;
            }
        });
    }
}

fn grid_lock(ui: &mut Ui, label: &str, locked: &mut bool) -> bool {
    use crate::{icons::Icon, timeline::icon_button};

    let icon = if *locked { Icon::Lock } else { Icon::Unlock };
    if icon_button(ui, label, Some(*locked), icon).clicked() {
        *locked = !*locked;
        true
    } else {
        false
    }
}

fn easing_fields(ui: &mut Ui, easing: &mut ArrayEasing, selected: Option<&mut PropertyStop>, has_successor: bool, has_filters: bool, has_path: bool) -> bool {
    fn mode_label(mode: ArrayEaseMode) -> &'static str {
        match mode {
            ArrayEaseMode::Global => tr!("Global"),
            ArrayEaseMode::PerSegment => tr!("Per Segment"),
        }
    }
    let before = easing.clone();
    let mut local_changed = false;
    form_row(ui, FORM_LABEL_DOCK, tr!("Mode"), |ui| {
        egui::ComboBox::from_id_salt("array_ease_mode")
            .width(144.0).truncate().selected_text(mode_label(easing.mode))
            .show_ui(ui, |ui| {
                for mode in [ArrayEaseMode::Global, ArrayEaseMode::PerSegment] {
                    ui.selectable_value(&mut easing.mode, mode, mode_label(mode));
                }
            }).response.named(tr!("Easing mode"));
    });
    form_row(ui, FORM_LABEL_DOCK, tr!("Ease"), |ui| {
        match easing.mode {
            ArrayEaseMode::Global => {
                if let Some(preset) = crate::ease_editor::named_picker(ui, "array_ease_preset", &easing.preset, 144.0, tr!("Ease")) {
                    easing.preset = preset;
                }
            }
            ArrayEaseMode::PerSegment => {
                let marker = selected.as_ref().map(|stop| stop.marker_id());
                let current = selected.as_ref().and_then(|stop| stop.value.outbound.as_ref()).unwrap_or(&Easing::Linear);
                let pick = ui.add_enabled_ui(has_successor, |ui| {
                    ui.push_id(marker, |ui| crate::ease_editor::named_picker(ui, "array_segment_preset", current, 144.0, tr!("Ease"))).inner
                });
                pick.response.on_disabled_hover_text(tr!("The final stop has no outgoing segment."));
                if let Some(preset) = pick.inner
                    && let Some(stop) = selected
                {
                    let next = (preset != Easing::Linear).then_some(preset);
                    local_changed = stop.value.outbound != next;
                    stop.value.outbound = next;
                }
            }
        }
    });
    ui.label(text(Type::Label, tr!("Apply to")));
    let labels = [tr!("Distribution"), tr!("Transform"), tr!("Colour Effect"), tr!("Filters")];
    let font = egui::TextStyle::Body.resolve(ui.style());
    let longest = labels.iter().map(|label| {
        ui.painter().layout_no_wrap((*label).into(), font.clone(), ui.visuals().text_color()).size().x
    }).fold(0.0, f32::max);
    let gap = ui.spacing().item_spacing.x;
    let columns = if 2.0 * (longest + crate::theme::TARGET) + gap <= ui.available_width() { 2 } else { 1 };
    let width = (ui.available_width() - (columns - 1) as f32 * gap) / columns as f32;
    egui::Grid::new("array_ease_targets").num_columns(columns).min_col_width(width).max_col_width(width)
        .show(ui, |ui| {
            let targets = [
                (&mut easing.distribution, (!has_path).then(|| tr!("Grid registration points stay evenly spaced."))),
                (&mut easing.transform, None), (&mut easing.colour_effect, None),
                (&mut easing.filters, (!has_filters).then(|| tr!("Available when the source has a filter."))),
            ];
            for (index, (label, (target, disabled_reason))) in labels.into_iter().zip(targets).enumerate() {
                if let Some(reason) = disabled_reason {
                    let mut off = false;
                    ui.add_enabled_ui(false, |ui| crate::theme::checkbox(ui, &mut off, label))
                        .inner.on_disabled_hover_text(reason);
                } else {
                    crate::theme::checkbox(ui, target, label);
                }
                if (index + 1).is_multiple_of(columns) {
                    ui.end_row();
                }
            }
        });
    local_changed || *easing != before
}

fn radial_fields(ui: &mut Ui, radial: &mut Radial) -> bool {
    use crate::{icons::Icon, timeline::icon_button};
    let previous = radial.closed_loop;
    ui.horizontal(|ui| {
        if icon_button(ui, tr!("Circle"), Some(previous), Icon::Oval).clicked() {
            radial.closed_loop = true;
        }
        if icon_button(ui, tr!("Spiral"), Some(!previous), Icon::Spiral).clicked() {
            radial.closed_loop = false;
        }
    });
    let mut changed = previous != radial.closed_loop;
    if changed && !radial.closed_loop {
        if radial.spiral_offset == 0.0 { radial.spiral_offset = 48.0; }
        if radial.sweep == 0.0 { radial.sweep = CLOSED_SWEEP_LIMIT; }
    }
    changed |= coordinates(ui, tr!("Centre"), [tr!("Centre X"), tr!("Centre Y")], &mut radial.centre, " px");
    let range = -RADIAL_FIELD_LIMIT..=RADIAL_FIELD_LIMIT;
    changed |= field(ui, tr!("Radius"), &mut radial.radius, " px", 0.0..=RADIAL_FIELD_LIMIT);
    changed |= field(ui, tr!("Start angle"), &mut radial.start, " °", range.clone());
    let limit = radial.sweep_limit().min(RADIAL_FIELD_LIMIT);
    if !previous && radial.closed_loop {
        radial.sweep = radial.sweep.clamp(-limit, limit);
    }
    changed |= field(ui, tr!("Sweep"), &mut radial.sweep, " °", -limit..=limit);
    if !radial.closed_loop {
        changed |= field(ui, tr!("Spiral Offset"), &mut radial.spiral_offset, tr!(" px/turn"), range);
        changed |= coordinates(ui, tr!("Centre Drift"), [tr!("Centre Drift X"), tr!("Centre Drift Y")],
            &mut radial.centre_drift, tr!(" px/turn"));
    }
    changed |= crate::theme::checkbox(ui, &mut radial.rotate, tr!("Rotate instances")).changed();
    changed
}

fn stop_properties(ui: &mut Ui, stop: &mut PropertyStop, source_filters: &[FilterEntry], open_filter_cards: &mut [bool], scale_lock: &mut bool, gesture: &mut Option<EditGesture>) -> bool {
    egui::Frame::new()
        .fill(T.panel_alt)
        .stroke(egui::Stroke::new(1.0, T.border))
        .corner_radius(T.radius_control)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let marker = stop.marker_id();
            let transform = section(ui, "array_stop_transform", Section::Rule, tr!("Transform"), None, |ui| {
                ui.push_id(("transform", marker), |ui| transform_stop(ui, &mut stop.value, scale_lock)).inner
            }).unwrap_or(false);
            let colour = section(ui, "array_stop_colour_effect", Section::Rule, tr!("Colour Effect"), None, |ui| {
                ui.push_id(("colour", marker), |ui| colour_stop(ui, &mut stop.value, gesture)).inner
            }).unwrap_or(false);
            let filters = !source_filters.is_empty() && section(ui, "array_stop_filters", Section::Rule, tr!("Filters"), None, |ui| {
                ui.push_id(("filters", marker), |ui| filter_stops(ui, source_filters, &mut stop.value.filters, open_filter_cards, gesture)).inner
            }).unwrap_or(false);
            transform || colour || filters
        }).inner
}

fn filter_stops(ui: &mut Ui, source: &[FilterEntry], filters: &mut [Filter], open_cards: &mut [bool], gesture: &mut Option<EditGesture>) -> bool {
    let mut filters = filters.iter_mut();
    let mut changed = false;
    for (slot, (entry, open)) in source.iter().zip(open_cards).enumerate() {
        let label = crate::i18n::t(entry.filter.label());
        let label = if source.len() == 1 { label.to_owned() } else { tr!("{filter} {slot}", filter = label, slot = slot + 1) };
        let filter = if ArrayPropertyStop::supports_filter(&entry.filter) { filters.next() } else { None };
        let heading = if entry.enabled { label.clone() } else { tr!("{filter} (off)", filter = label.as_str()) };
        ui.push_id(slot, |ui| {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, T.border))
                .corner_radius(T.radius_control)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    changed |= crate::theme::section_with_state(ui, Section::Rule, &heading, open, |ui| {
                        let Some(filter) = filter else {
                            ui.label(text(Type::Caption, tr!("Uses source settings")));
                            return false;
                        };
                        let mut values = filter.clone();
                        let edited = if let Filter::Blur { blur_x, blur_y, .. } = &mut values {
                            ui.horizontal_wrapped(|ui| {
                                crate::panels::filter_blur_fields(ui, &label, blur_x, blur_y)
                            }).inner && blur_x.is_finite() && blur_y.is_finite()
                        } else {
                            let mut edit = None;
                            let read_only = !entry.enabled && matches!(values, Filter::AdjustColor { .. });
                            let mut rows = |ui: &mut Ui| {
                                crate::panels::effect_filter_rows(ui, crate::panels::FilterRows::Array, &label, &mut values, &mut edit)
                            };
                            let popup = if read_only { ui.add_enabled_ui(false, rows).inner } else { rows(ui) };
                            if let Some(popup) = popup {
                                *gesture = Some(EditGesture::Picker(ui.ctx().viewport_id(), popup));
                            }
                            edit.is_some()
                        };
                        if edited { *filter = values.sanitized(); }
                        edited
                    }).unwrap_or(false);
                });
        });
    }
    changed
}

fn colour_stop(ui: &mut Ui, stop: &mut ArrayPropertyStop, gesture: &mut Option<EditGesture>) -> bool {
    let before = (stop.color, stop.alpha);
    let current = stop.color.style();
    let picked = form_row(ui, FORM_LABEL_DOCK, tr!("Style"), |ui| {
        crate::panels::colour_style_picker(ui, "array_colour_style", tr!("Colour effect style"), current, false)
    });
    if let Some(style) = picked.filter(|style| *style != current) {
        stop.color = stop.color.to_style(style);
    }
    match &mut stop.color {
        ColorEffect::Advanced(transform) => {
            let [red, green, blue, alpha] = crate::panels::advanced_colour_channels();
            for (i, (label, mul_name, off_name, _)) in [(3, alpha), (0, red), (1, green), (2, blue)] {
                let multiplier = if i == 3 { &mut stop.alpha } else { &mut transform.mul[i] };
                let mut mv = *multiplier * 100.0;
                let mut ov = transform.add[i] * 255.0;
                let (mul, add) = form_row(ui, FORM_LABEL_DOCK, label, |ui| {
                    crate::panels::advanced_colour_fields(ui, &mut mv, &mut ov, mul_name, off_name)
                });
                if mul && mv.is_finite() { *multiplier = mv / 100.0; }
                if add && ov.is_finite() { transform.add[i] = ov / 255.0; }
            }
            return before != (stop.color, stop.alpha);
        }
        ColorEffect::Brightness(value) => {
            colour_percent(ui, tr!("Brightness"), value, -100.0..=100.0);
        }
        ColorEffect::Tint { color, amount } => {
            let mut rgb = crate::panels::rgb(*color);
            form_row(ui, FORM_LABEL_DOCK, tr!("Tint"), |ui| {
                let response = crate::colour::button(ui, &mut rgb).named(tr!("Tint colour"));
                if response.changed() {
                    *color = Rgba::hex(u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]));
                    *gesture = Some(EditGesture::Picker(ui.ctx().viewport_id(), crate::colour::swatch_popup_id(response.id)));
                }
                ui.label(text(Type::Data, crate::panels::hex(rgb)));
            });
            colour_percent(ui, tr!("Tint amount"), amount, 0.0..=100.0);
        }
        _ => {}
    }
    stop.alpha = stop.alpha.clamp(0.0, 1.0);
    colour_percent(ui, tr!("Alpha"), &mut stop.alpha, 0.0..=100.0);
    before != (stop.color, stop.alpha)
}

fn colour_percent(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>) {
    let mut percent = *value * 100.0;
    if dimension(ui, label, &mut percent, range, " %", true) && percent.is_finite() {
        *value = percent / 100.0;
    }
}

fn transform_stop(ui: &mut Ui, stop: &mut ArrayPropertyStop, scale_lock: &mut bool) -> bool {
    let mut changed = with_type(ui, Type::Data, |ui| ui.horizontal_wrapped(|ui| {
        ui.label(text(Type::Label, tr!("Scale")));
        let [mut x, mut y] = stop.scale.map(|value| value * 100.0);
        let old = (x, y);
        let limit = 100.0 * ArrayPropertyStop::SCALE_LIMIT;
        let value = |ui: &mut Ui, value: &mut f32, axis: &str, name: &str| {
            ui.add(egui::DragValue::new(value).range(-limit..=limit).clamp_existing_to_range(false)
                .prefix(axis).suffix(" %").speed(0.5).max_decimals(1)).named(name).changed()
        };
        let x_changed = value(ui, &mut x, "X ", tr!("Scale X"));
        let y_changed = value(ui, &mut y, "Y ", tr!("Scale Y"));
        let icon = if *scale_lock { crate::icons::Icon::Lock } else { crate::icons::Icon::Unlock };
        if crate::timeline::icon_button_wide(ui, tr!("Lock scale"), Some(*scale_lock), icon, 20.0).clicked() {
            *scale_lock = !*scale_lock;
        }
        if x_changed || y_changed {
            if *scale_lock { (x, y) = crate::panels::linked_scale(old, (x, y), x_changed); }
            stop.scale = [x / 100.0, y / 100.0];
        }
        x_changed || y_changed
    }).inner);
    let limit = ArrayPropertyStop::ROTATION_LIMIT;
    changed |= dimension(ui, tr!("Rotation"), &mut stop.rotation, -limit..=limit, "°", true);
    changed
}

fn linear_dimensions(ui: &mut Ui, endpoints: &mut Endpoints, count: u32) -> bool {
    let [dx, dy] = endpoints.extent();
    let mut length = dx.hypot(dy);
    let mut direction = if length > 0.0 { dy.atan2(dx).to_degrees() } else { 0.0 };
    let mut changed = dimension(ui, tr!("Direction"), &mut direction, -180.0..=180.0, "°", length > 0.0);
    changed |= dimension(ui, tr!("Length"), &mut length, 0.0..=1_000_000.0, " px", true);
    if changed {
        let (sin, cos) = direction.to_radians().sin_cos();
        endpoints.end = [endpoints.start[0] + length * cos, endpoints.start[1] + length * sin];
    }
    let spacing = endpoints.spacing(count);
    for (axis, label) in [tr!("Spacing X"), tr!("Spacing Y")].into_iter().enumerate() {
        let mut distance = spacing.map_or(0.0, |v| v[axis]);
        if dimension(ui, label, &mut distance, -1_000_000.0..=1_000_000.0, " px", spacing.is_some()) {
            endpoints.end[axis] = endpoints.start[axis] + distance * count.saturating_sub(1) as f32;
            changed = true;
        }
    }
    changed
}

fn dimension(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str, enabled: bool) -> bool {
    form_row(ui, FORM_LABEL_DOCK, label, |ui| dimension_value(ui, label, value, range, suffix, enabled))
}

fn dimension_value(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str, enabled: bool) -> bool {
    with_type(ui, Type::Data, |ui| {
        let field = egui::DragValue::new(value).range(range).clamp_existing_to_range(false).suffix(suffix).max_decimals(1);
        let field = if enabled { field } else { field.custom_formatter(|_, _| "—".to_owned()).suffix("") };
        ui.add_enabled(enabled, field).named(label).changed()
    })
}

fn coordinates(ui: &mut Ui, label: &str, [x_name, y_name]: [&str; 2], [x, y]: &mut [f32; 2], suffix: &str) -> bool {
    form_row(ui, FORM_LABEL_DOCK, label, |ui| {
        let mut changed = false;
        for (value, axis, name) in [(x, "X ", x_name), (y, "Y ", y_name)] {
            changed |= with_type(ui, Type::Data, |ui| {
                ui.add(egui::DragValue::new(value).range(-100_000.0..=100_000.0).clamp_existing_to_range(false)
                    .prefix(axis).suffix(suffix).max_decimals(1))
            }).named(name).changed();
        }
        changed
    })
}

fn field(ui: &mut Ui, label: &str, value: &mut f32, suffix: &str, range: std::ops::RangeInclusive<f32>) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(text(Type::Label, label));
        with_type(ui, Type::Data, |ui| {
            ui.add(
                egui::DragValue::new(value)
                    .range(range)
                    .clamp_existing_to_range(false)
                    .suffix(suffix)
                    .speed(1.0)
                    .max_decimals(1),
            )
        })
        .named(label)
        .changed()
    })
    .inner
}

#[cfg(all(test, gpu_stage))]
pub(crate) use tests::test_window_device;

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, Vec2, pos2, vec2};
    use egui::accesskit::{Action, ActionData, ActionRequest, Node, NodeId, Role, TreeId};

    fn instance(s: &mut Session, x: f32, name: &str) {
        s.execute("draw.rect", &json!({"x": x, "y": 50, "w": 24, "h": 14})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": name})).unwrap();
    }

    #[test]
    fn switches_replay_from_the_source_and_commit_once() {
        let mut s = Session::default();
        instance(&mut s, 50.0, "Source");
        let source = s.selection.first().unwrap();
        let (doc, selection, steps) = (s.doc.clone(), s.selection.clone(), s.history_len());
        let mut state = State::default();
        assert_eq!(state.mode(), None);
        state.select_mode(&mut s, Some(Mode::Linear));
        assert_eq!(s.selection.len(), 10);
        assert_eq!(s.selection.first(), Some(source));
        let settings = state.settings.as_mut().unwrap();
        settings.linear.end = [200.0, 170.0];
        settings.radial.radius = 60.0;
        settings.count = 7;
        settings.stops.insert(1, PropertyStop::new(2, 0.4, 0.25));
        state.refresh(&mut s);
        let linear = s.doc.clone();
        for _ in 0..3 {
            state.select_mode(&mut s, Some(Mode::Radial));
            assert!(s.doc != linear);
            assert_eq!(s.selection.len(), 7);
            assert_eq!(s.selection.first(), Some(source));
            state.select_mode(&mut s, Some(Mode::Linear));
            assert!(s.doc == linear, "replaying Linear keeps its spacing and stops");
        }
        assert_eq!(s.history_len(), steps);
        state.finish(&mut s, true);
        assert_eq!(state.mode(), None);
        assert_eq!(s.selection.len(), 7);
        assert_eq!(s.history_len(), steps + 1);
        assert_eq!(s.undo_label(), Some("Array"));
        assert!(state.settings.is_none());
        s.undo().unwrap();
        assert!(s.doc == doc && s.selection == selection);
    }

    #[test]
    fn none_preserves_the_draft_for_the_same_source_and_starts_fresh_on_a_new_source() {
        let mut s = Session::default();
        instance(&mut s, 50.0, "First");
        let first = s.selection.first().unwrap();
        let doc = s.doc.clone();
        let steps = s.history_len();
        let mut state = State::default();
        state.select_mode(&mut s, Some(Mode::Linear));
        state.settings.as_mut().unwrap().count = 3;
        state.refresh(&mut s);
        let remembered = state.settings.clone();
        state.select_mode(&mut s, None);
        assert!(s.doc == doc && s.selection.first() == Some(first) && !s.in_gesture());
        assert_eq!(s.history_len(), steps);
        assert_eq!(state.settings, remembered);
        state.select_mode(&mut s, Some(Mode::Linear));
        assert_eq!(s.selection.len(), 3);
        assert_eq!(state.settings, remembered);
        state.select_mode(&mut s, None);

        instance(&mut s, 400.0, "Second");
        let second = s.selection.first().unwrap();
        let origin = source_origin(&s).unwrap();
        let next_doc = s.doc.clone();
        state.select_mode(&mut s, Some(Mode::Radial));
        assert_eq!(s.selection.first(), Some(second));
        assert_eq!(s.selection.len(), 10);
        assert_eq!(state.settings.as_ref(), Some(&Settings::at(origin)));
        state.settings.as_mut().unwrap().radial.spiral_offset = 30.0;
        assert!(state.customized());
        state.reset(&mut s);
        assert!(!state.customized());
        assert!(s.array_preview_active() && s.selection.len() == 10);
        state.finish(&mut s, false);
        assert!(s.doc == next_doc && s.selection.first() == Some(second));
    }

    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) -> Vec<(NodeId, Node)> {
        frame_at(ctx, app, events, None)
    }

    fn frame_at(ctx: &egui::Context, app: &mut App, events: Vec<Event>, time: Option<f64>) -> Vec<(NodeId, Node)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 940.0))),
            events,
            time,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.frame(ui));
        out.textures_delta.clear();
        out.platform_output.accesskit_update.map(|u| u.nodes).unwrap_or_default()
    }

    fn nodes(ctx: &egui::Context, app: &mut App) -> Vec<(NodeId, Node)> {
        frame(ctx, app, vec![]);
        frame(ctx, app, vec![])
    }

    fn click(ctx: &egui::Context, app: &mut App, node: &Node) {
        let rect = node.bounds().unwrap();
        let pos = egui::pos2(((rect.x0 + rect.x1) / 2.0) as f32, ((rect.y0 + rect.y1) / 2.0) as f32);
        frame(ctx, app, vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            frame(ctx, app, vec![Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
    }

    fn key(key: Key, pressed: bool) -> Event {
        Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
    }

    fn linear_app() -> (App, egui::Context) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut s = Session::default();
        instance(&mut s, 180.0, "Source");
        let mut app = App::with_context(&ctx, s, Some(std::path::PathBuf::from("unused.png")));
        app.show_panel(Panel::Array);
        for group in &mut app.dock.right.groups {
            group.folded = !group.tabs.contains(&Panel::Array);
        }
        (app, ctx)
    }

    fn commit_control(ctx: &egui::Context, app: &mut App, label: &str) -> Node {
        // The commit controls are the last section of the dock's scrollable content.
        frame(ctx, app, vec![Event::PointerMoved(pos2(1450.0, 300.0)), Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -3000.0),
            modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel,
        }]);
        nodes(ctx, app).into_iter().find(|(_, n)| n.label() == Some(label))
            .unwrap_or_else(|| panic!("missing {label}")).1
    }

    #[test]
    fn timeline_output_is_shared_and_cancel_and_validation_keep_the_spatial_draft() {
        let (mut app, ctx) = linear_app();
        app.s.doc.stage.fps = 30.0;
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let pick = |app: &mut App, name| {
            let node = if app.modal_open() {
                nodes(&ctx, app).into_iter().find(|(_, n)| n.label() == Some(name))
                    .unwrap_or_else(|| panic!("missing {name}")).1
            } else {
                commit_control(&ctx, app, name)
            };
            click(&ctx, app, &node);
        };
        assert_eq!(app.array_panel.settings.as_ref().unwrap().commit_as, CommitAs::Spatial);
        pick(&mut app, "Create Unique Symbols");
        pick(&mut app, "Timeline Animation");
        assert_eq!(app.array_panel.settings.as_ref().unwrap().commit_as, CommitAs::Timeline);
        assert!(!nodes(&ctx, &mut app).iter().any(|(_, n)| n.label() == Some("Create Unique Symbols")));
        pick(&mut app, "Spatial Array");
        assert!(app.array_panel.settings.as_ref().unwrap().create_unique_symbols);
        pick(&mut app, "Timeline Animation");
        for label in ["Commit", "Cancel"] {
            assert_eq!(nodes(&ctx, &mut app).iter().filter(|(_, n)| n.label() == Some(label)).count(), 1);
        }

        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert!(!app.array_panel.can_commit());
        assert!(app.array_panel.step(&mut app.s, false));
        assert_eq!(app.array_panel.mode(), Some(Mode::Linear));
        assert_eq!(app.array_panel.settings.as_ref().unwrap().commit_as, CommitAs::Timeline);
        assert!(app.array_panel.step(&mut app.s, false));
        assert_eq!(app.array_panel.settings.as_ref().unwrap().commit_as, CommitAs::Spatial);
        assert!(app.array_panel.step(&mut app.s, true));

        let (preview, draft, steps) = (app.s.doc.clone(), app.array_panel.settings.clone(), app.s.history_len());
        pick(&mut app, "Commit");
        assert!(app.modal_open());
        assert_eq!(app.array_panel.timeline_config.as_ref().unwrap().frames, 30);
        assert!(nodes(&ctx, &mut app).iter().any(|(_, n)| n.label() == Some("1.00 seconds")));
        app.array_panel.timeline_config.as_mut().unwrap().frames = 45;
        assert!(nodes(&ctx, &mut app).iter().any(|(_, n)| n.label() == Some("1.50 seconds")));
        assert!(!app.array_panel.timeline_config.as_ref().unwrap().hold);
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        assert!(!app.modal_open());
        assert!(app.s.array_preview_active() && app.array_panel.can_step(false));
        assert!(app.s.doc == preview && app.array_panel.settings == draft);

        app.array_panel.request_commit(&mut app.s, false);
        let config = app.array_panel.timeline_config.as_mut().unwrap();
        assert_eq!(config.frames, 45);
        config.hold = true;
        config.frames = 2;
        pick(&mut app, "Create Animation");
        assert!(app.array_panel.timeline_config.as_ref().unwrap().error.as_ref().unwrap().to_string().contains("10 frames"));
        assert!(app.s.doc == preview && app.array_panel.settings == draft && app.s.history_len() == steps);

        pick(&mut app, "Use 10 frames");
        assert_eq!(app.array_panel.timeline_config.as_ref().unwrap().frames, 10);
        assert!(app.array_panel.timeline_config.as_ref().unwrap().error.is_none());
        assert!(app.s.array_preview_active());
        pick(&mut app, "Create Animation");
        assert!(!app.s.array_preview_active() && !app.modal_open());
        assert_eq!(app.s.history_len(), steps + 1);
        assert_eq!(app.s.selection.len(), 1);
    }

    #[test]
    fn timeline_modal_title_drags_and_close_preserves_draft_like_cancel() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let before = app.array_panel.before_edit(None);
        app.array_panel.settings.as_mut().unwrap().commit_as = CommitAs::Timeline;
        app.array_panel.record_edit(before, None);
        let draft = app.array_panel.settings.clone();
        let doc = app.s.doc.clone();
        let steps = app.s.history_len();
        let find = |app: &mut App, name| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label() == Some(name)).unwrap_or_else(|| panic!("missing {name}")).1;
        let center = |node: &Node| {
            let r = node.bounds().unwrap();
            pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32)
        };

        app.array_panel.request_commit(&mut app.s, false);
        app.array_panel.timeline_config.as_mut().unwrap().frames = 61;
        let from = center(&find(&mut app, "Create Timeline Animation"));
        let close = center(&find(&mut app, "Close"));
        let duration = center(&find(&mut app, "Duration (frames)"));
        let area = egui::AreaState::load(&ctx, Id::unique("array_timeline")).unwrap().rect();
        assert!(from.y - area.top() < 36.0, "title {from:?}, modal {area:?}");
        assert!((close.y - from.y).abs() < 3.0 && close.x > from.x, "title {from:?}, close {close:?}");
        assert!((0.0..52.0).contains(&(duration.y - from.y)), "title {from:?}, duration {duration:?}");
        assert!(area.height() < 230.0, "modal {area:?}");
        begin_drag(&ctx, &mut app, from);
        for step in [vec2(35.0, 20.0), vec2(95.0, 50.0)] {
            frame(&ctx, &mut app, vec![Event::PointerMoved(from + step)]);
        }
        frame(&ctx, &mut app, vec![pointer_button(from + vec2(95.0, 50.0), false)]);
        let moved = center(&find(&mut app, "Create Timeline Animation"));
        assert!(moved.x > from.x + 30.0 && moved.y > from.y + 15.0, "{from:?} -> {moved:?}");

        for button in ["Close", "Cancel"] {
            let action = find(&mut app, button);
            click(&ctx, &mut app, &action);
            assert!(!app.modal_open());
            assert!(app.s.array_preview_active() && app.array_panel.can_step(false));
            assert!(app.s.doc == doc && app.array_panel.settings == draft);
            assert_eq!(app.s.history_len(), steps);
            app.array_panel.request_commit(&mut app.s, false);
            assert_eq!(app.array_panel.timeline_config.as_ref().unwrap().frames, 61);
        }
    }

    #[test]
    fn circle_timeline_offers_native_motion_and_keeps_the_draft_on_cancel() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        let choice = commit_control(&ctx, &mut app, "Timeline Animation");
        click(&ctx, &mut app, &choice);
        assert!(app.array_panel.can_commit());
        app.array_panel.settings.as_mut().unwrap().stops.insert(1, PropertyStop::new(2, 0.5, 0.4));
        app.array_panel.refresh(&mut app.s);
        let draft = app.array_panel.settings.clone();
        let preview = app.s.doc.clone();
        let commit = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &commit);
        let modal = nodes(&ctx, &mut app);
        let frames = app.array_panel.timeline_config.as_ref().unwrap().frames;
        assert!(modal.iter().any(|(_, n)| n.label() == Some("Motion Tween")));
        assert!(!modal.iter().any(|(_, n)| n.label() == Some("Classic Tween") || n.label() == Some("Hold Keyframes")));
        let close = modal.into_iter().find(|(_, n)| n.label() == Some("Close")).unwrap().1;
        click(&ctx, &mut app, &close);
        assert!(!app.modal_open());
        assert!(app.s.array_preview_active() && app.array_panel.can_step(false));
        assert!(app.s.doc == preview && app.array_panel.settings == draft);
        app.array_panel.request_commit(&mut app.s, false);
        assert_eq!(app.array_panel.timeline_config.as_ref().unwrap().frames, frames);
        app.array_panel.timeline_config.as_mut().unwrap().frames = 2;
        let pick = |app: &mut App, label| {
            let action = nodes(&ctx, app).into_iter().find(|(_, n)| n.label() == Some(label)).unwrap().1;
            click(&ctx, app, &action);
        };
        pick(&mut app, "Create Animation");
        assert!(app.s.doc == preview && app.s.array_preview_active());
        pick(&mut app, "Use 3 frames");
        assert_eq!(app.array_panel.timeline_config.as_ref().unwrap().frames, 3);
        assert!(app.s.array_preview_active());
        pick(&mut app, "Create Animation");
        assert!(!app.modal_open() && !app.s.array_preview_active());
        let layer = app.s.timeline().layer(app.s.layer).unwrap();
        let key = layer.keyframe_at(app.s.frame).unwrap();
        assert!(key.motion.as_ref().is_some_and(|motion| motion.path.is_some()));
        assert_eq!(key.elements.len(), 1);
        assert_eq!(key.duration, 3);
    }

    #[test]
    fn spiral_timeline_uses_the_shared_modal_and_retains_unsupported_drafts() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        let settings = app.array_panel.settings.as_mut().unwrap();
        settings.radial.closed_loop = false;
        settings.radial.spiral_offset = 48.0;
        settings.radial.centre_drift = [24.0, -18.0];
        settings.radial.sweep = -36_000.0;
        app.array_panel.refresh(&mut app.s);
        let choice = commit_control(&ctx, &mut app, "Timeline Animation");
        click(&ctx, &mut app, &choice);
        assert!(app.array_panel.can_commit());

        let click_named = |app: &mut App, label| {
            let action = nodes(&ctx, app).into_iter().find(|(_, node)| node.label() == Some(label)).unwrap().1;
            click(&ctx, app, &action);
        };
        let draft = app.array_panel.settings.clone();
        let preview = app.s.doc.clone();
        app.array_panel.request_commit(&mut app.s, false);
        click_named(&mut app, "Create Animation");
        assert!(app.array_panel.timeline_config.as_ref().unwrap().error.as_ref().is_some());
        assert!(app.s.array_preview_active() && app.s.doc == preview && app.array_panel.settings == draft);
        click_named(&mut app, "Cancel");
        assert!(!app.modal_open());

        app.array_panel.settings.as_mut().unwrap().radial.sweep = -720.0;
        app.array_panel.refresh(&mut app.s);
        app.array_panel.request_commit(&mut app.s, false);
        click_named(&mut app, "Create Animation");
        assert!(!app.modal_open() && !app.s.array_preview_active());
        let layer = app.s.timeline().layer(app.s.layer).unwrap();
        let key = layer.keyframe_at(app.s.frame).unwrap();
        assert!(key.motion.as_ref().is_some_and(|motion| motion.path.is_some()));
        assert_eq!(key.elements.len(), 1);
    }

    fn screen(app: &App, [x, y]: [f32; 2]) -> Pos2 {
        let zoom = app.view.zoom;
        let size = vec2(app.s.doc.stage.width as f32, app.s.doc.stage.height as f32);
        let origin = app.stage_cache.canvas.center() + app.view.pan - size * zoom * 0.5;
        pos2(origin.x.round() + x * zoom, origin.y.round() + y * zoom)
    }

    fn pointer_button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn filtered_app(count: u32) -> (App, egui::Context) {
        let (mut app, ctx) = linear_app();
        screenshot(&mut app, "array-blur");
        app.prefs.render_mode = crate::prefs::RenderMode::Immediate;
        app.view.fit = false;
        app.view.zoom = 0.65;
        app.array_panel.settings.as_mut().unwrap().count = count;
        app.array_panel.refresh(&mut app.s);
        assert_eq!(app.s.selection.len(), count as usize);
        assert!(app.s.selected_elements().iter().all(|(_, el)| {
            matches!(el.kind.filters(), Some([FilterEntry { enabled: true, filter: Filter::Blur { quality: 2, .. } }]))
        }));
        nodes(&ctx, &mut app);
        assert_eq!(app.stage_cache.drawn_by.as_ref().map(|(gpu, _)| *gpu), Some(false));
        (app, ctx)
    }

    fn wait_for_stage(ctx: &egui::Context, app: &mut App, ready: impl Fn(&App) -> bool) {
        let started = std::time::Instant::now();
        while !ready(app) {
            assert!(started.elapsed().as_secs() < 15, "the stage did not settle");
            std::thread::sleep(std::time::Duration::from_millis(4));
            frame(ctx, app, vec![]);
        }
    }

    #[cfg(gpu_stage)]
    #[test]
    #[ignore = "requires a native hardware GPU; exercises the real shared-device stage"]
    fn high_quality_shadow_glow_gpu_drag_lifecycle() {
        high_quality_filter_gpu_drag_lifecycle(vec![json!({"type": "drop_shadow", "quality": 3}), json!({"type": "glow", "quality": 3})]);
    }

    #[cfg(gpu_stage)]
    #[test]
    #[ignore = "requires a native hardware GPU; exercises the real shared-device stage"]
    fn high_quality_bevel_stack_gpu_drag_lifecycle() {
        high_quality_filter_gpu_drag_lifecycle(high_quality_bevel_stack());
    }

    #[cfg(gpu_stage)]
    #[test]
    #[ignore = "requires a native hardware GPU; exercises the real shared-device stage"]
    fn high_quality_gradient_glow_stack_gpu_drag_lifecycle() {
        high_quality_filter_gpu_drag_lifecycle(high_quality_gradient_glow_stack());
    }

    #[cfg(gpu_stage)]
    #[test]
    #[ignore = "requires a native hardware GPU; exercises the real shared-device stage"]
    fn high_quality_gradient_bevel_stack_gpu_drag_lifecycle() {
        high_quality_filter_gpu_drag_lifecycle(high_quality_gradient_bevel_stack());
    }

    #[cfg(gpu_stage)]
    #[test]
    #[ignore = "requires a native hardware GPU; exercises the real shared-device stage"]
    fn high_quality_all_filter_families_gpu_drag_lifecycle() {
        let mut filters = high_quality_gradient_bevel_stack();
        filters.push(json!({"type": "adjust_color", "brightness": 8, "contrast": 16, "saturation": -20, "hue": -65}));
        high_quality_filter_gpu_drag_lifecycle(filters);
    }

    #[cfg(gpu_stage)]
    fn high_quality_gradient_bevel_stack() -> Vec<Value> {
        let mut filters = high_quality_gradient_glow_stack();
        filters.push(json!({"type": "gradient_bevel", "side": "full", "quality": 3, "blur_x": 10, "blur_y": 8,
            "stops": [{"offset": 0, "color": "#f2cf58ff"}, {"offset": 0.32, "color": "#e7875ca0"},
                {"offset": 0.5, "color": "#00000000"}, {"offset": 0.8, "color": "#513189a0"}, {"offset": 1, "color": "#234362ff"}]}));
        filters
    }

    #[cfg(gpu_stage)]
    fn high_quality_gradient_glow_stack() -> Vec<Value> {
        let mut filters = high_quality_bevel_stack();
        filters.push(json!({"type": "gradient_glow", "side": "full", "quality": 3, "blur_x": 9, "blur_y": 11,
            "stops": [{"offset": 0, "color": "#ffffff00"}, {"offset": 0.3, "color": "#6231dd88"}, {"offset": 1, "color": "#20d8bfff"}]}));
        filters
    }

    #[cfg(gpu_stage)]
    fn high_quality_bevel_stack() -> Vec<Value> {
        vec![json!({"type": "blur", "blur_x": 3, "blur_y": 2, "quality": 3}), json!({"type": "drop_shadow", "quality": 3}),
            json!({"type": "glow", "quality": 3}), json!({"type": "bevel", "side": "full", "quality": 3})]
    }

    #[cfg(gpu_stage)]
    pub(crate) fn test_window_device() -> eframe::egui_wgpu::RenderState {
        use eframe::egui_wgpu::{RenderState, Renderer, wgpu};
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let info = adapter.get_info();
        assert_ne!(info.device_type, wgpu::DeviceType::Cpu);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let target_format = wgpu::TextureFormat::Rgba8Unorm;
        let renderer = Renderer::new(&device, target_format, Default::default());
        RenderState {
            adapter, instance, device: device.clone(), queue, target_format, available_adapters: vec![],
            renderer: Arc::new(egui::mutex::RwLock::new(renderer)),
        }
    }

    #[cfg(gpu_stage)]
    fn high_quality_filter_gpu_drag_lifecycle(filters: Vec<Value>) {
        use eframe::egui_wgpu::wgpu;
        let textures = test_window_device();
        let device = textures.device.clone();
        let info = textures.adapter.get_info();
        let (mut app, ctx) = linear_app();
        app.stage_cache.gpu.set_window_device(Some(&textures));
        app.prefs.stage_renderer = crate::prefs::StageRenderer::Gpu;
        app.prefs.render_mode = crate::prefs::RenderMode::Auto;
        app.view.fit = false;
        app.view.zoom = 1.0;
        app.s.execute("element.translate", &json!({"dx": 100, "dy": 230})).unwrap();
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": filters})).unwrap();
        let base = app.s.doc.clone();
        let steps = app.s.history_len();
        let settled = |app: &App| app.stage_cache.ready.0 == app.stage_cache.ready.1
            && app.stage_cache.ready.0 > 0 && matches!(app.stage_cache.drawn_by, Some((true, _)))
            && app.stage_cache.service.as_ref().is_some_and(|svc| !svc.busy());
        // Some backends compile their pipelines much more slowly on a cold start. Keep
        // that startup separate from the deadline and timings for live preview frames.
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        let starting = std::time::Instant::now();
        loop {
            match app.stage_cache.gpu.state() {
                crate::gpu_stage::State::Running { direct: true, .. } => break,
                bad @ (crate::gpu_stage::State::Failed(_) | crate::gpu_stage::State::Stopped { .. } | crate::gpu_stage::State::Running { direct: false, .. }) => {
                    panic!("the {:?} shared stage could not start: {bad:?}", info.backend);
                }
                _ => {}
            }
            assert!(starting.elapsed().as_secs() < 90, "the {:?} shared stage did not start", info.backend);
            app.shot.as_mut().unwrap().frames = 0;
            frame(&ctx, &mut app, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let mut high_water = 0;
        let mut late_water = 0;
        let mut frames = Vec::new();
        for round in 0..8 {
            app.shot.as_mut().unwrap().frames = 0;
            app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
            wait_for_stage(&ctx, &mut app, settled);
            assert_eq!(app.s.selection.len(), 10);
            app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
            wait_for_stage(&ctx, &mut app, settled);
            let linear = app.array_panel.settings.as_ref().unwrap().linear;
            let middle = std::array::from_fn(|axis| (linear.start[axis] + linear.end[axis]) * 0.5);
            let from = screen(&app, middle);
            drag_gizmo(&ctx, &mut app, from, vec2(8.0, 10.0), false);
            assert!(app.array_panel.gizmo_drag.is_some());
            for step in 1..=10 {
                let now = std::time::Instant::now();
                let delta = if step % 2 == 0 { 12.0 * step as f32 } else { -60.0 * step as f32 };
                let to = from + vec2(delta, -80.0);
                frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
                frames.push(now.elapsed().as_secs_f64() * 1000.0);
                if let Some(report) = device.generate_allocator_report() {
                    high_water = high_water.max(report.total_allocated_bytes);
                    if round >= 4 { late_water = late_water.max(report.total_allocated_bytes); }
                }
            }
            let end = from + vec2(8.0, 3.0);
            frame(&ctx, &mut app, vec![Event::PointerMoved(end), pointer_button(end, false)]);
            wait_for_stage(&ctx, &mut app, settled);
            let moved = app.array_panel.settings.as_ref().unwrap().linear;
            assert_ne!(moved.start, linear.start);
            for axis in 0..2 {
                assert!(((moved.end[axis] - linear.end[axis]) - (moved.start[axis] - linear.start[axis])).abs() < 0.001);
            }
            let (images, _, _, _) = app.stage_cache.service.as_ref().unwrap().cache_stats();
            assert!(images <= app.stage_cache.ready.1, "only the current preview tiles remain cached");
            assert_eq!(app.s.committed_document().0, &base);
        }
        let preview = app.s.doc.clone();
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_ne!(app.s.doc, preview);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(app.s.doc, preview);
        app.array_panel.finish(&mut app.s, false);
        assert_eq!(app.s.doc, base);
        assert_eq!(app.s.history_len(), steps);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(app.s.doc, preview);
        app.array_panel.finish(&mut app.s, true);
        assert_eq!(app.s.history_len(), steps + 1);
        app.s.undo().unwrap();
        assert_eq!(app.s.doc, base);
        app.s.redo().unwrap();
        assert_eq!(app.s.doc, preview);
        wait_for_stage(&ctx, &mut app, settled);
        eframe::App::on_exit(&mut app, None);
        drop(app);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        frames.sort_by(f64::total_cmp);
        eprintln!("{} ({:?}): 8 mode switches and 80 bar moves, p50 {:.2} ms, p95 {:.2} ms",
            info.name, info.backend, frames[40], frames[75]);
        if high_water > 0 {
            eprintln!("live allocations peak {:.1} MiB, latter half {:.1} MiB", high_water as f64 / 1048576.0, late_water as f64 / 1048576.0);
        } else {
            eprintln!("allocator report unavailable");
        }
    }

    #[cfg(gpu_stage)]
    #[test]
    #[ignore = "requires a hardware adapter; measures rapid Quantity changes with real stage textures"]
    fn filtered_quantity_churn_across_array_modes() {
        use eframe::egui_wgpu::wgpu;

        let textures = test_window_device();
        let device = &textures.device;
        let info = textures.adapter.get_info();
        eprintln!("Quantity pressure on {} ({:?})", info.name, info.backend);
        for (label, mode, closed, filtered) in [
            ("plain Spiral", Mode::Radial, false, false),
            ("filtered Linear", Mode::Linear, true, true),
            ("filtered Circle", Mode::Radial, true, true),
            ("filtered Spiral", Mode::Radial, false, true),
        ] {
            let (mut app, ctx) = linear_app();
            app.stage_cache.gpu.set_window_device(Some(&textures));
            app.prefs.stage_renderer = crate::prefs::StageRenderer::Gpu;
            app.prefs.render_mode = crate::prefs::RenderMode::Background;
            app.view.fit = false;
            app.view.zoom = 1.0;
            app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
            if filtered {
                app.s.execute("element.set_filters", &json!({"filters": [
                    {"type":"glow","blur_x":16,"blur_y":12,"quality":3},
                    {"type":"drop_shadow","blur_x":14,"blur_y":10,"distance":12,"quality":3}
                ]})).unwrap();
            }
            let base = app.s.doc.clone();
            let history = app.s.history_len();
            app.array_panel.select_mode(&mut app.s, Some(mode));
            let settings = app.array_panel.settings.as_mut().unwrap();
            settings.count = 16;
            settings.linear = Endpoints { start: [230.0, 280.0], end: [580.0, 340.0] };
            settings.radial.centre = [410.0, 290.0];
            settings.radial.radius = 65.0;
            settings.radial.closed_loop = closed;
            settings.radial.sweep = if closed { -360.0 } else { -720.0 };
            settings.radial.spiral_offset = 28.0;
            settings.radial.centre_drift = [15.0, -10.0];
            settings.easing.mode = ArrayEaseMode::PerSegment;
            settings.stops.first_mut().unwrap().value.outbound = Some(Easing::QuadIn);
            let mut middle = PropertyStop::new(2, 0.5, 1.0);
            middle.value = settings.stops.first().unwrap().value.clone();
            middle.value.offset = 0.5;
            middle.value.outbound = Some(Easing::QuadOut);
            settings.stops.insert(1, middle);
            app.array_panel.refresh(&mut app.s);

            let started = std::time::Instant::now();
            while !matches!(app.stage_cache.gpu.state(), crate::gpu_stage::State::Running { direct: true, .. }) {
                assert!(started.elapsed().as_secs() < 90, "{label}: {:?}", app.stage_cache.gpu.state());
                app.shot.as_mut().unwrap().frames = 0;
                frame(&ctx, &mut app, vec![]);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let live_bytes = || device.generate_allocator_report().map_or(0, |r| r.total_allocated_bytes);
            let mut peaks = [0, 0];
            for peak in &mut peaks {
                for count in [120, 410, 260, 500, 330, 490, 500] {
                    // Drive the same count change and refresh as the Quantity control.
                    app.array_panel.settings.as_mut().unwrap().count = count;
                    app.array_panel.refresh(&mut app.s);
                    app.shot.as_mut().unwrap().frames = 0;
                    frame(&ctx, &mut app, vec![]);
                    assert_eq!(app.s.selection.len(), count as usize);
                    assert_eq!(app.s.committed_document().0, &base);
                    *peak = (*peak).max(live_bytes());
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                wait_for_stage(&ctx, &mut app, |app| {
                    app.stage_cache.shown_state == Some(app.s.state)
                        && app.stage_cache.service.as_ref().is_some_and(|svc| !svc.busy())
                });
                *peak = (*peak).max(live_bytes());
                assert!(matches!(app.stage_cache.gpu.state(), crate::gpu_stage::State::Running { direct: true, .. }));
            }
            let draft = app.array_panel.settings.clone();
            app.array_panel.finish(&mut app.s, false);
            assert!(app.s.doc == base && app.s.history_len() == history);
            app.array_panel.select_mode(&mut app.s, Some(mode));
            assert!(app.array_panel.settings == draft && app.s.selection.len() == 500);
            eframe::App::on_exit(&mut app, None);
            drop(app);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            eprintln!("{label}: sampled live peaks {:?} bytes; after drop and fence {} bytes", peaks, live_bytes());
        }
    }

    fn warm_filtered(ctx: &egui::Context, app: &mut App) {
        app.prefs.render_mode = crate::prefs::RenderMode::Auto;
        wait_for_stage(ctx, app, |app| app.stage_cache.filtered_array_warmed());
    }

    fn begin_drag(ctx: &egui::Context, app: &mut App, from: Pos2) {
        frame(ctx, app, vec![Event::PointerMoved(from)]);
        frame(ctx, app, vec![pointer_button(from, true)]);
    }

    fn blur_x_grip(ctx: &egui::Context, app: &mut App) -> Pos2 {
        for label in ["Transform", "Colour Effect"] {
            let header = nodes(ctx, app).into_iter().find(|(_, node)| node.label() == Some(label)).unwrap().1;
            click(ctx, app, &header);
        }
        if !app.array_panel.open_filter_cards.first().copied().unwrap_or(false) {
            let header = nodes(ctx, app).into_iter().find(|(_, node)| node.label() == Some("Blur")).unwrap().1;
            click(ctx, app, &header);
        }
        let field = nodes(ctx, app).into_iter().find(|(_, node)| {
            node.role() == Role::SpinButton && node.label() == Some("Blur blur X")
        }).unwrap().1;
        let bounds = field.bounds().unwrap();
        pos2(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32)
    }

    #[test]
    fn filtered_linear_drag_tracks_reversals_and_settles_to_exact_tiles() {
        use anicraft_engine::geom::Affine;
        use anicraft_engine::render_service::{make_job, render_job};

        let (mut app, ctx) = filtered_app(10);
        let mut ends = app.array_panel.gizmo_endpoints().unwrap();
        ends.end = [624.0, 218.0];
        app.array_panel.set_endpoints(&mut app.s, ends);
        nodes(&ctx, &mut app);
        warm_filtered(&ctx, &mut app);
        let first = app.s.state;
        let generation = app.s.array_translation_generation();
        let history = app.s.history_len();
        let from = screen(&app, ends.end);
        begin_drag(&ctx, &mut app, from);
        for shift in [19.25, -12.5, 34.75, -21.25, 42.5] {
            frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(shift, 0.5))]);
            assert_eq!(app.stage_cache.filtered_array_presented(), Some(app.s.state));
            assert_eq!(app.stage_cache.shown_state, Some(first));
            assert_eq!(app.s.array_translation_generation(), generation);
        }
        wait_for_stage(&ctx, &mut app, |app| app.stage_cache.shown_state == Some(app.s.state));
        assert!(app.array_panel.gizmo_drag.is_some());
        assert_eq!(app.stage_cache.filtered_array_presented(), None);

        let to = from + vec2(-29.25, -3.5);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        assert_eq!(app.stage_cache.filtered_array_presented(), Some(app.s.state));
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        wait_for_stage(&ctx, &mut app, |app| app.stage_cache.shown_state == Some(app.s.state));
        let selection: Vec<_> = {
            let mut ids: Vec<_> = app.s.selection.ids().collect();
            ids.sort_unstable();
            ids
        };
        let view = anicraft_render::View { transform: Affine::scale(app.view.zoom, app.view.zoom), width: 416, height: 260, background: Some(app.s.doc.stage.background) };
        let job = make_job(app.s.state, &app.s.doc, 0, view, None).dim_around(Some(selection.into())).preview_tile(true);
        let shown = app.stage_cache.service.as_ref().unwrap().cached(job.key).unwrap();
        assert_eq!(shown.rgba_premul, render_job(&job).rgba_premul);

        let preview = app.s.doc.clone();
        let before_commit = anicraft_render::render_frame(&preview, 0, 1.0).unwrap().to_rgba8();
        let commit = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &commit);
        assert!(!app.s.array_preview_active() && !app.stage_cache.filtered_array_warmed());
        assert_eq!(app.s.history_len(), history + 1);
        assert_eq!(before_commit, anicraft_render::render_frame(&app.s.doc, 0, 1.0).unwrap().to_rgba8());
    }

    #[test]
    fn filtered_linear_preview_drops_provisional_pictures_on_edits_and_cancel() {
        let (mut app, ctx) = filtered_app(10);
        app.array_panel.finish(&mut app.s, false);
        let source = app.s.doc.clone();
        let history = app.s.history_len();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let blur = blur_x_grip(&ctx, &mut app);
        warm_filtered(&ctx, &mut app);
        let initial = app.s.doc.clone();
        let move_end = |app: &mut App| {
            let from = screen(app, app.array_panel.gizmo_endpoints().unwrap().end);
            let to = from + vec2(31.5, -16.25);
            begin_drag(&ctx, app, from);
            frame(&ctx, app, vec![Event::PointerMoved(to)]);
            assert_eq!(app.stage_cache.filtered_array_presented(), Some(app.s.state));
            to
        };
        let to = move_end(&mut app);
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert!(app.s.doc == initial);
        assert_eq!(app.stage_cache.filtered_array_presented(), None);
        assert!(app.stage_cache.filtered_array_warmed());

        let generation = app.s.array_translation_generation();
        begin_drag(&ctx, &mut app, blur);
        let to = blur + vec2(36.0, 0.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        assert_ne!(app.s.array_translation_generation(), generation);
        assert_eq!(app.stage_cache.filtered_array_presented(), None);
        assert!(!app.stage_cache.filtered_array_warmed());
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        warm_filtered(&ctx, &mut app);
        app.view.zoom = 0.8;
        frame(&ctx, &mut app, vec![]);
        assert!(!app.stage_cache.filtered_array_warmed());
        warm_filtered(&ctx, &mut app);
        let to = move_end(&mut app);
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        frame(&ctx, &mut app, vec![key(Key::Escape, false), pointer_button(to, false)]);
        assert!(!app.s.array_preview_active() && !app.stage_cache.filtered_array_warmed());
        assert_eq!(app.stage_cache.filtered_array_presented(), None);
        assert!(app.s.doc == source && app.s.history_len() == history);
        wait_for_stage(&ctx, &mut app, |app| app.stage_cache.shown_state == Some(app.s.state));
    }

    #[test]
    fn filter_stop_edits_invalidate_warmed_blur_and_active_effects_use_exact_tiles() {
        use anicraft_engine::doc::FilterParam;
        let (mut app, ctx) = filtered_app(10);
        app.array_panel.finish(&mut app.s, false);
        let mut filters = json!([
            {"type": "blur", "blur_x": 8, "blur_y": 8, "quality": 2},
            {"type": "drop_shadow", "distance": 14, "enabled": false},
            {"type": "glow", "color": "#3296ee", "enabled": false},
            {"type": "bevel", "side": "full", "distance": 5, "enabled": false},
        ]);
        app.s.execute("element.set_filters", &json!({"filters": filters})).unwrap();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        warm_filtered(&ctx, &mut app);
        for (slot, param, value) in [(1, FilterParam::Distance, 48.0), (2, FilterParam::Strength, 240.0), (3, FilterParam::Distance, 18.0)] {
            let generation = app.s.array_translation_generation();
            let settings = app.array_panel.settings.as_mut().unwrap();
            assert!(settings.stops[0].value.filters[slot].set_param(param, value));
            app.array_panel.refresh(&mut app.s);
            frame(&ctx, &mut app, vec![]);
            assert_ne!(app.s.array_translation_generation(), generation);
            assert!(!app.stage_cache.filtered_array_warmed());
            assert!(app.stage_cache.filtered_array_presented().is_none());
            warm_filtered(&ctx, &mut app);
        }

        app.array_panel.finish(&mut app.s, false);
        filters[1]["enabled"] = json!(true);
        filters[2]["enabled"] = json!(true);
        filters[3]["enabled"] = json!(true);
        app.s.execute("element.set_filters", &json!({"filters": filters})).unwrap();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(app.array_panel.settings.as_ref().unwrap().stops[0].value.filters[3].param(FilterParam::Distance), Some(5.0));
        for _ in 0..12 { frame(&ctx, &mut app, vec![]); }
        let old = app.s.array_translation_generation();
        assert!(old.is_some());
        let new = app.array_panel.settings.as_mut().unwrap();
        assert!(new.stops[0].value.filters[3].set_param(FilterParam::Angle, 220.0));
        app.array_panel.refresh(&mut app.s);
        assert_ne!(app.s.array_translation_generation(), old);
        assert!(!app.stage_cache.filtered_array_warmed());
        let from = screen(&app, app.array_panel.gizmo_endpoints().unwrap().end);
        let to = from + vec2(27.5, 8.25);
        begin_drag(&ctx, &mut app, from);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        assert!(app.stage_cache.filtered_array_presented().is_none());
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        wait_for_stage(&ctx, &mut app, |app| app.stage_cache.shown_state == Some(app.s.state));
        assert!(app.stage_cache.filtered_array_presented().is_none());
        assert!(!app.stage_cache.filtered_array_warmed());
    }

    #[test]
    #[ignore = "local Array drag timing on the CPU stage"]
    fn measure_linear_drag() {
        use std::time::Instant;
        fn median(mut samples: Vec<f64>) -> f64 {
            samples.sort_by(f64::total_cmp);
            samples[samples.len() / 2]
        }
        for count in [10, 200] {
            let (mut app, ctx) = linear_app();
            screenshot(&mut app, "array");
            app.prefs.render_mode = crate::prefs::RenderMode::Immediate;
            app.array_panel.settings.as_mut().unwrap().count = count;
            app.array_panel.refresh(&mut app.s);
            app.view.fit = false;
            app.view.zoom = 0.65;
            nodes(&ctx, &mut app);
            let initial = app.array_panel.gizmo_endpoints().unwrap();
            for (region, end_x) in [("stage", 480.0), ("pasteboard", 720.0)] {
                let mut ends = initial;
                ends.end[0] = end_x;
                app.array_panel.set_endpoints(&mut app.s, ends);
                nodes(&ctx, &mut app);
                let from = screen(&app, ends.end);
                assert!(app.stage_cache.canvas.contains(from));
                frame(&ctx, &mut app, vec![Event::PointerMoved(from)]);
                frame(&ctx, &mut app, vec![pointer_button(from, true)]);
                let mut wall = Vec::new();
                let mut render = Vec::new();
                let mut to = from;
                for i in 1..=12 {
                    to = from + vec2(i as f32 * 3.0, i as f32 * 0.75);
                    let timer = Instant::now();
                    frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
                    wall.push(timer.elapsed().as_secs_f64() * 1_000.0);
                    render.push(f64::from(app.stage_cache.ms));
                }
                near(app.array_panel.gizmo_endpoints().unwrap().end, [end_x + 36.0 / 0.65, ends.end[1] + 9.0 / 0.65]);
                frame(&ctx, &mut app, vec![pointer_button(to, false)]);
                let mut update = Vec::new();
                let mut selection = Vec::new();
                for i in 1..=12 {
                    ends.end[0] -= i as f32;
                    let timer = Instant::now();
                    app.array_panel.set_endpoints(&mut app.s, ends);
                    update.push(timer.elapsed().as_secs_f64() * 1_000.0);
                    let timer = Instant::now();
                    std::hint::black_box(app.s.selection_bounds());
                    selection.push(timer.elapsed().as_secs_f64() * 1_000.0);
                }
                eprintln!("Array {count:3} {region:10} median ms: frame {:.3}, render {:.3}, update {:.3}, selection bounds {:.3}",
                    median(wall), median(render), median(update), median(selection));
            }
        }
    }

    #[test]
    #[ignore = "local filtered Array preview timing on the CPU stage"]
    fn measure_filtered_preview() {
        use std::time::{Duration, Instant};

        const WARMUP: usize = 6;
        const SAMPLES: usize = 30;

        fn percentiles(samples: &mut [f64]) -> (f64, f64) {
            samples.sort_by(f64::total_cmp);
            let median = (samples[(samples.len() - 1) / 2] + samples[samples.len() / 2]) / 2.0;
            (median, samples[(samples.len() * 95).div_ceil(100) - 1])
        }

        let drag = |app: &mut App, ctx: &egui::Context, count, interaction, from: Pos2, step: Vec2, rebuilds| {
            frame(ctx, app, vec![Event::PointerMoved(from)]);
            frame(ctx, app, vec![pointer_button(from, true)]);
            let mut wall = Vec::with_capacity(SAMPLES);
            let mut render = Vec::with_capacity(SAMPLES);
            let mut replays = 0;
            let mut to = from;
            for i in 1..=WARMUP + SAMPLES {
                to = from + step * i as f32;
                let generation = app.s.array_translation_generation().unwrap();
                let state = app.s.state;
                // Immediate mode doesn't use this value to choose when to render. Reset it
                // so a sample with no render batch can't report the preceding batch's time.
                app.stage_cache.ms = 0.0;
                let started = Instant::now();
                frame(ctx, app, vec![Event::PointerMoved(to)]);
                if i > WARMUP {
                    wall.push(started.elapsed().as_secs_f64() * 1_000.0);
                    assert_ne!(app.s.state, state, "the pointer sample must edit the preview");
                    assert_eq!(app.stage_cache.ready.0, app.stage_cache.ready.1);
                    assert!(app.stage_cache.ms > 0.0, "the sample must render a fresh batch");
                    render.push(f64::from(app.stage_cache.ms));
                    replays += usize::from(app.s.array_translation_generation().unwrap() != generation);
                }
            }
            frame(ctx, app, vec![pointer_button(to, false)]);
            assert_eq!(replays, if rebuilds { SAMPLES } else { 0 });
            let (p50, p95) = percentiles(&mut wall);
            let (render50, render95) = percentiles(&mut render);
            eprintln!("{count:3} {interaction:16} frame {p50:7.2}/{p95:7.2} ms  stage {render50:7.2}/{render95:7.2} ms  replays {replays}/{SAMPLES}");
        };

        eprintln!("Filtered Array CPU Immediate, p50/p95, {WARMUP} warm-up + {SAMPLES} measured pointer moves");
        for count in [10, 50, 200] {
            let (mut app, ctx) = filtered_app(count);

            for (interaction, end_x) in [("End on stage", 480.0), ("End on pasteboard", 720.0)] {
                let mut ends = app.array_panel.gizmo_endpoints().unwrap();
                ends.end = [end_x, 220.0];
                app.array_panel.set_endpoints(&mut app.s, ends);
                nodes(&ctx, &mut app);
                let from = screen(&app, ends.end);
                let step = vec2(2.0, 0.5);
                assert!(app.stage_cache.canvas.contains(from + step * (WARMUP + SAMPLES) as f32));
                drag(&mut app, &ctx, count, interaction, from, step, false);
                near(app.array_panel.gizmo_endpoints().unwrap().end, [
                    ends.end[0] + step.x * (WARMUP + SAMPLES) as f32 / app.view.zoom,
                    ends.end[1] + step.y * (WARMUP + SAMPLES) as f32 / app.view.zoom,
                ]);
            }

            let mut ends = app.array_panel.gizmo_endpoints().unwrap();
            ends.end = [480.0, 220.0];
            app.array_panel.set_endpoints(&mut app.s, ends);
            let from = blur_x_grip(&ctx, &mut app);
            drag(&mut app, &ctx, count, "Blur X", from, vec2(3.0, 0.0), true);
            assert!(app.array_panel.settings.as_ref().unwrap().stops[1].value.filters[0].param(anicraft_engine::doc::FilterParam::BlurX) > Some(22.0));
        }

        eprintln!("Filtered Array CPU Auto, 60 Hz pointer moves with one repaint between moves");
        let auto_drag = |count, reuse: bool, interaction, end_x| {
            let (mut app, ctx) = filtered_app(count);
            let mut ends = app.array_panel.gizmo_endpoints().unwrap();
            ends.end = [end_x, 220.0];
            app.array_panel.set_endpoints(&mut app.s, ends);
            nodes(&ctx, &mut app);
            let blur = interaction == "Blur X";
            let from = match interaction {
                "Blur X" => blur_x_grip(&ctx, &mut app),
                "Bar" => screen(&app, [(ends.start[0] + ends.end[0]) / 2.0, (ends.start[1] + ends.end[1]) / 2.0]),
                _ => screen(&app, ends.end),
            };
            let step = if blur { vec2(3.0, 0.0) } else { vec2(2.0, 0.5) };
            app.stage_cache.disable_filtered_array = !reuse;
            app.prefs.render_mode = crate::prefs::RenderMode::Auto;
            let epoch = ctx.input(|i| i.time);
            let clock = Instant::now();
            let step_frame = |app: &mut App, events| frame_at(&ctx, app, events, Some(epoch + clock.elapsed().as_secs_f64()));
            if reuse {
                while !app.stage_cache.filtered_array_warmed() {
                    assert!(clock.elapsed().as_secs() < 15, "the provisional preview did not warm");
                    step_frame(&mut app, vec![]);
                    std::thread::sleep(Duration::from_millis(4));
                }
            }
            let warm_ms = clock.elapsed().as_secs_f64() * 1_000.0;
            step_frame(&mut app, vec![Event::PointerMoved(from)]);
            step_frame(&mut app, vec![pointer_button(from, true)]);
            let beat = Duration::from_secs_f64(1.0 / 60.0);
            let (mut exact, mut cached, mut partial) = (0, 0, 0);
            let mut times = Vec::with_capacity(SAMPLES);
            let mut to = from;
            for i in 1..=WARMUP + SAMPLES {
                let started = Instant::now();
                to = from + step * i as f32;
                let previous = app.s.state;
                step_frame(&mut app, vec![Event::PointerMoved(to)]);
                let pointer_ms = started.elapsed().as_secs_f64() * 1_000.0;
                let provisional = app.stage_cache.filtered_array_presented();
                let ready = app.stage_cache.ready.0;
                std::thread::sleep((beat / 2).saturating_sub(started.elapsed()));
                step_frame(&mut app, vec![]);
                if i > WARMUP {
                    assert_ne!(app.s.state, previous);
                    times.push(pointer_ms);
                    if app.stage_cache.shown_state == Some(app.s.state) {
                        exact += 1;
                    } else if provisional == Some(app.s.state) || app.stage_cache.filtered_array_presented() == Some(app.s.state) {
                        cached += 1;
                    } else if ready.max(app.stage_cache.ready.0) > 0 {
                        partial += 1;
                    }
                }
                std::thread::sleep(beat.saturating_sub(started.elapsed()));
            }
            let catch_up = |app: &mut App, started: Instant| {
                while app.stage_cache.shown_state != Some(app.s.state) {
                    assert!(started.elapsed().as_secs() < 15, "the stage did not settle");
                    std::thread::sleep(Duration::from_millis(4));
                    step_frame(app, vec![]);
                }
                started.elapsed().as_secs_f64() * 1_000.0
            };
            let held_ms = catch_up(&mut app, Instant::now());
            assert_eq!(app.array_panel.gizmo_drag.is_some(), !blur);
            to += step;
            step_frame(&mut app, vec![Event::PointerMoved(to)]);
            let released = Instant::now();
            step_frame(&mut app, vec![pointer_button(to, false)]);
            let released_ms = catch_up(&mut app, released);
            if !reuse || blur { assert_eq!(cached, 0); } else { assert_eq!(exact + cached, SAMPLES); }
            let (p50, p95) = percentiles(&mut times);
            let mode = if reuse { "cached" } else { "original" };
            eprintln!("{count:3} {interaction:12} {mode:8} pointer {p50:6.2}/{p95:6.2} ms; {exact:2} exact, {cached:2} reused, {partial:2} partial of {SAMPLES}; held {held_ms:6.2} ms, released {released_ms:6.2} ms, warm {warm_ms:6.2} ms");
        };
        for count in [10, 50, 200] {
            for (interaction, end_x) in [("End", 480.0), ("Cross stage", 620.0), ("Bar", 480.0), ("Blur X", 480.0)] {
                for reuse in [false, true] { auto_drag(count, reuse, interaction, end_x); }
            }
        }
    }

    #[test]
    #[ignore = "local cached Blur translation probe against the CPU oracle"]
    fn measure_cached_blur_translation() {
        use anicraft_engine::geom::Affine;
        use anicraft_render::{Pixmap, View, render};
        use std::time::Instant;

        const WIDTH: u32 = 640;
        const HEIGHT: u32 = 400;
        const SAMPLES: usize = 30;
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 24, "y": 16, "w": 34, "h": 15, "fill": "#ce523f", "stroke": null})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Blur cache probe", "kind": "movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters": [{"type": "blur", "blur_x": 22, "blur_y": 3, "quality": 2}]})).unwrap();
        let source = s.selected_elements()[0].1.transform.translation();
        let view = View { transform: Affine::IDENTITY, width: WIDTH, height: HEIGHT, background: None };
        let started = Instant::now();
        let (patch, _) = render(&s.display_list(), &View { width: 88, height: 48, ..view }).unwrap();
        eprintln!("Cached Blur translation CPU, one {}x{} isolated image built in {:.2} ms", patch.width, patch.height, started.elapsed().as_secs_f64() * 1_000.0);

        // Test-only source-over of the padded premultiplied patch in document order. The
        // rectangle fits for all measured moves. Measure composition, including canvas clear.
        let composite = |out: &mut Pixmap, count: usize, start_x: usize, spacing: usize| {
            out.data.fill([0.0; 4]);
            for i in 0..count {
                let left = start_x + i * spacing;
                for (y, row) in patch.data.chunks_exact(patch.width as usize).enumerate() {
                    let at = (128 + y) * WIDTH as usize + left;
                    for (dst, src) in out.data[at..at + row.len()].iter_mut().zip(row) {
                        let keep = 1.0 - src[3];
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d = s + *d * keep;
                        }
                    }
                }
            }
        };

        for count in [10, 50, 200] {
            let spacing = 480 / (count - 1);
            let mut out = Pixmap::new(WIDTH, HEIGHT).unwrap();
            let mut full_times = Vec::with_capacity(SAMPLES);
            let mut cached_times = Vec::with_capacity(SAMPLES);
            let mut params = json!({"count": count, "start_x": source.x, "start_y": source.y + 128.0, "dx": spacing, "dy": 0});
            for x in 0..SAMPLES + 6 {
                params["start_x"] = json!(source.x + x as f32);
                s.preview_array(&params).unwrap();
                let list = s.display_list();
                let started = Instant::now();
                let (fresh, _) = render(&list, &view).unwrap();
                let full_ms = started.elapsed().as_secs_f64() * 1_000.0;
                let started = Instant::now();
                composite(&mut out, count, x, spacing);
                let cached_ms = started.elapsed().as_secs_f64() * 1_000.0;
                assert!(out.to_rgba8() == fresh.to_rgba8(), "whole-pixel placement of {count} copies at offset {x}");
                if x >= 6 {
                    full_times.push(full_ms);
                    cached_times.push(cached_ms);
                }
            }
            full_times.sort_by(f64::total_cmp);
            cached_times.sort_by(f64::total_cmp);
            eprintln!("{count:3} full/cached p50 {:.2}/{:.2} ms; all whole-pixel renders byte-identical",
                (full_times[14] + full_times[15]) / 2.0, (cached_times[14] + cached_times[15]) / 2.0);

            let x = SAMPLES + 5;
            params["start_x"] = json!(source.x + x as f32 + 0.5);
            s.preview_array(&params).unwrap();
            let (fractional, _) = render(&s.display_list(), &view).unwrap();
            let cached_rgba = out.to_rgba8();
            let fresh_rgba = fractional.to_rgba8();
            let changed = cached_rgba.as_chunks::<4>().0.iter().zip(fresh_rgba.as_chunks::<4>().0)
                .filter(|(a, b)| a != b).count();
            assert!(changed > 0, "an integer reuse is not the final half-pixel render");
            eprintln!("{count:3} half-pixel reference differs from integer reuse in {changed} pixels");
            s.finish_array_preview(false);
        }
    }

    #[test]
    #[ignore = "local spiral drag timing on the CPU stage"]
    fn measure_radial_drag() {
        use std::time::Instant;
        for count in [10, 200] {
            let (mut app, ctx) = linear_app();
            screenshot(&mut app, "array-radial-open:720,18");
            app.prefs.render_mode = crate::prefs::RenderMode::Immediate;
            app.view.fit = false;
            app.view.zoom = 0.65;
            for (region, x) in [("stage", 330.0), ("pasteboard", 690.0)] {
                let settings = app.array_panel.settings.as_mut().unwrap();
                settings.count = count;
                settings.radial.centre[0] = x;
                settings.radial.sweep = 700.0;
                app.array_panel.refresh(&mut app.s);
                nodes(&ctx, &mut app);
                let end = app.array_panel.settings.as_ref().unwrap().radial.point(1.0);
                let from = screen(&app, [end.x, end.y]);
                assert!(app.stage_cache.canvas.contains(from));
                frame(&ctx, &mut app, vec![Event::PointerMoved(from)]);
                frame(&ctx, &mut app, vec![pointer_button(from, true)]);
                let mut wall = Vec::new();
                let mut to = from;
                for i in 1..=12 {
                    to = from + vec2(i as f32 * 2.5, i as f32 * 1.5);
                    let timer = Instant::now();
                    frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
                    wall.push(timer.elapsed().as_secs_f64() * 1_000.0);
                }
                assert!(app.array_panel.gizmo_drag.is_some());
                let moved = app.array_panel.settings.as_ref().unwrap().radial;
                assert_ne!(moved.sweep, 700.0);
                frame(&ctx, &mut app, vec![pointer_button(to, false)]);
                wall.sort_by(f64::total_cmp);
                eprintln!("Spiral {count:3} {region:10} median frame ms {:.3}", wall[wall.len() / 2]);
            }
        }
    }

    fn drag_gizmo(ctx: &egui::Context, app: &mut App, from: Pos2, by: Vec2, finish: bool) {
        frame(ctx, app, vec![Event::PointerMoved(from)]);
        frame(ctx, app, vec![pointer_button(from, true)]);
        frame(ctx, app, vec![Event::PointerMoved(from + by)]);
        if finish {
            frame(ctx, app, vec![pointer_button(from + by, false)]);
        }
    }

    fn set_number(ctx: &egui::Context, app: &mut App, target_node: NodeId, value: f64) {
        frame(ctx, app, vec![Event::AccessKitActionRequest(ActionRequest {
            action: Action::SetValue, target_tree: TreeId::ROOT, target_node, data: Some(ActionData::NumericValue(value)),
        })]);
    }

    fn history_key(ctx: &egui::Context, app: &mut App, key: Key, extra: Modifiers) {
        let modifiers = Modifiers::COMMAND | extra;
        frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        frame(ctx, app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
    }

    #[test]
    fn grid_numeric_fields_and_independent_locks_update_the_live_gizmo() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        let current = |app: &App| app.array_panel.settings.as_ref().unwrap().grid;
        let find = |app: &mut App, label| nodes(&ctx, app).into_iter()
            .find(|(_, node)| node.label() == Some(label)).unwrap_or_else(|| panic!("Grid control: {label}"));
        let set = |app: &mut App, label, value| {
            let id = find(app, label).0;
            set_number(&ctx, app, id, value);
        };
        let initial = current(&app);
        assert_eq!(initial.geometry.counts, [3, 3]);
        assert!(!initial.counts_locked && !initial.geometry.spacing_locked);
        assert!(initial.flip_on_reverse);

        set(&mut app, "Columns", 5.0);
        let grid = current(&app);
        assert_eq!(grid.geometry.counts, [5, 3]);
        assert_eq!(grid.geometry.endpoints(), initial.geometry.endpoints());
        near(grid.geometry.spacing, [30.0, 60.0]);
        set(&mut app, "Row Spacing (Y)", -35.0);
        let grid = current(&app);
        near(grid.geometry.endpoints().extent(), [120.0, -70.0]);
        assert_eq!(app.s.selected_elements().len(), 15);
        let last = app.s.selected_elements().pop().unwrap().1;
        near([last.transform.m[4], last.transform.m[5]], grid.geometry.endpoints().end);

        let (_, count_lock) = find(&mut app, "Lock row and column counts");
        click(&ctx, &mut app, &count_lock);
        assert_eq!(current(&app).geometry.counts, [5, 5]);
        assert_eq!(current(&app).geometry.endpoints(), grid.geometry.endpoints());
        set(&mut app, "Rows", 25.0);
        assert_eq!(current(&app).geometry.counts, [22, 22]);
        assert_eq!(app.s.selected_elements().len(), 484);
        set(&mut app, "Rows", 4.0);
        let (_, count_lock) = find(&mut app, "Lock row and column counts");
        click(&ctx, &mut app, &count_lock);
        let (_, spacing_lock) = find(&mut app, "Lock spacing");
        click(&ctx, &mut app, &spacing_lock);
        let grid = current(&app);
        assert!(!grid.counts_locked && grid.geometry.spacing_locked);
        assert!((grid.geometry.spacing[0] + grid.geometry.spacing[1]).abs() < 1e-4);
        set(&mut app, "Columns", 3.0);
        set(&mut app, "Column Spacing (X)", 42.0);
        let grid = current(&app);
        assert_eq!(grid.geometry.counts, [3, 4]);
        near(grid.geometry.spacing, [42.0, -42.0]);
        near(grid.geometry.endpoints().extent(), [84.0, -126.0]);

        let (_, flip) = nodes(&ctx, &mut app).into_iter()
            .find(|(_, node)| node.label() == Some("Flip on Reverse") && node.role() == Role::Button).unwrap();
        click(&ctx, &mut app, &flip);
        let last = app.s.selected_elements().pop().unwrap().1;
        assert!(last.transform.m[3] > 0.0);
        assert_eq!(current(&app).geometry.endpoints(), grid.geometry.endpoints());
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(current(&app), grid);
    }

    #[test]
    fn grid_property_distribution_buttons_keep_draft_geometry_and_history() {
        let (mut app, ctx) = linear_app();
        screenshot(&mut app, "array-grid");
        app.array_panel.settings.as_mut().unwrap().stops.last_mut().unwrap().value.alpha = 0.25;
        app.array_panel.refresh(&mut app.s);
        let initial = app.array_panel.settings.clone().unwrap();
        let original = app.s.doc.clone();
        assert_eq!(initial.grid.property_distribution, GridPropertyDistribution::Both);
        let choice = |app: &mut App, label| {
            nodes(&ctx, app).into_iter()
                .find(|(_, node)| node.role() == Role::Button && node.label() == Some(label))
                .unwrap().1
        };
        let both = choice(&mut app, "X + Y");
        assert_eq!(both.toggled(), Some(egui::accesskit::Toggled::True));
        let x = choice(&mut app, "X");
        click(&ctx, &mut app, &x);
        let mut expected = initial.clone();
        expected.grid.property_distribution = GridPropertyDistribution::X;
        assert_eq!(app.array_panel.settings.as_ref(), Some(&expected));
        assert_ne!(app.s.doc, original);
        let y = choice(&mut app, "Y");
        click(&ctx, &mut app, &y);
        expected.grid.property_distribution = GridPropertyDistribution::Y;
        assert_eq!(app.array_panel.settings.as_ref(), Some(&expected));
        assert_eq!(choice(&mut app, "X").toggled(), Some(egui::accesskit::Toggled::False));
        assert_eq!(choice(&mut app, "Y").toggled(), Some(egui::accesskit::Toggled::True));
        let preview = app.s.doc.clone();
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().grid.property_distribution, GridPropertyDistribution::X);
        history_key(&ctx, &mut app, Key::Z, Modifiers::SHIFT);
        assert_eq!(app.s.doc, preview);
        app.array_panel.finish(&mut app.s, false);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert_eq!(app.array_panel.settings.as_ref(), Some(&expected));
        assert_eq!(app.s.doc, preview);
        app.array_panel.finish(&mut app.s, true);
        assert_eq!(app.s.doc, preview);
    }

    #[test]
    fn array_create_unique_symbols_is_shared_undoable_and_only_touches_library_at_commit() {
        let (mut app, ctx) = linear_app();
        let base = app.s.doc.clone();
        let base_ids = app.s.selection.clone();
        let steps = app.s.history_len();
        let checkbox = |app: &mut App| commit_control(&ctx, app, "Create Unique Symbols");
        let unique = |app: &App| app.array_panel.settings.as_ref().unwrap().create_unique_symbols;
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let preview = app.s.doc.clone();
        assert!(!unique(&app));
        let toggle = checkbox(&mut app);
        click(&ctx, &mut app, &toggle);
        assert!(unique(&app));
        assert_eq!(app.s.doc, preview);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert!(!unique(&app));
        history_key(&ctx, &mut app, Key::Z, Modifiers::SHIFT);
        assert!(unique(&app));

        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert!(unique(&app));
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert!(unique(&app));
        assert_eq!(app.s.doc.library, base.library);
        let cancel = commit_control(&ctx, &mut app, "Cancel");
        click(&ctx, &mut app, &cancel);
        assert_eq!(app.s.doc, base);
        assert_eq!(app.s.selection, base_ids);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert!(unique(&app));
        let generation = app.s.array_translation_generation();
        let preview = app.s.doc.clone();
        let toggle = checkbox(&mut app);
        click(&ctx, &mut app, &toggle);
        assert_eq!(app.s.array_translation_generation(), generation);
        assert_eq!(app.s.doc, preview);
        let toggle = checkbox(&mut app);
        click(&ctx, &mut app, &toggle);
        let commit = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &commit);
        assert!(!app.s.array_preview_active());
        assert_eq!(app.s.doc.library.len(), base.library.len() + 9);
        assert_eq!(app.s.history_len(), steps + 1);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.s.doc, base);
        assert_eq!(app.s.selection, base_ids);
    }

    #[test]
    fn grid_stagger_controls_keep_the_base_gizmo_and_recover_the_draft() {
        let (mut app, ctx) = linear_app();
        screenshot(&mut app, "array-grid");
        let get = |app: &App| app.array_panel.settings.as_ref().unwrap().grid;
        let find = |app: &mut App, label| {
            nodes(&ctx, app).into_iter().find(|(_, node)| node.label() == Some(label))
                .unwrap_or_else(|| panic!("Grid control: {label}"))
        };
        let initial = get(&app);
        assert_eq!(initial.stagger, GridStagger::default());
        let start = initial.geometry.start;
        let initial_preview = app.s.doc.clone();
        // Collapse the other section so all stagger controls fit in the test viewport.
        let (_, distribution) = find(&mut app, "Distribution");
        click(&ctx, &mut app, &distribution);
        let (_, stagger) = find(&mut app, "Stagger");
        click(&ctx, &mut app, &stagger);
        let (_, rows) = find(&mut app, "Offset Rows");
        click(&ctx, &mut app, &rows);
        let (_, columns) = find(&mut app, "Offset Columns");
        click(&ctx, &mut app, &columns);
        let (frequency, _) = find(&mut app, "Column offset Every N");
        set_number(&ctx, &mut app, frequency, 3.0);
        let (column_y, _) = find(&mut app, "Column Y Offset");
        set_number(&ctx, &mut app, column_y, -20.0);
        let repeated = get(&app);
        assert_eq!(repeated.stagger.columns.every.get(), 3);
        assert_eq!(repeated.stagger.columns.offset, [0.0, -20.0]);
        assert!(repeated.stagger.rows.enabled && repeated.stagger.columns.enabled);
        assert_eq!(repeated.geometry, initial.geometry);
        let check_position = |app: &App, index: usize, expected| {
            let copies = app.s.selected_elements();
            let at = &copies[index].1;
            near([at.transform.m[4], at.transform.m[5]], expected);
        };
        check_position(&app, 0, start);
        check_position(&app, 6, [start[0] + 2.0 * 84.0 + 50.0, start[1] + 70.0 - 20.0]);
        check_position(&app, 11, [start[0] + 3.0 * 84.0, start[1] + 2.0 * 70.0]);
        assert_ne!(app.s.doc, initial_preview);

        let (_, accumulate) = find(&mut app, "Accumulate");
        click(&ctx, &mut app, &accumulate);
        let accumulated = get(&app);
        assert!(accumulated.stagger.accumulate);
        check_position(&app, 11, [start[0] + 3.0 * 84.0 + 50.0, start[1] + 2.0 * 70.0 - 20.0]);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(get(&app), repeated);
        history_key(&ctx, &mut app, Key::Z, Modifiers::SHIFT);
        assert_eq!(get(&app), accumulated);

        let (_, distribution) = find(&mut app, "Distribution");
        click(&ctx, &mut app, &distribution);
        let (rows, _) = find(&mut app, "Rows");
        set_number(&ctx, &mut app, rows, 2.0);
        let shorter = get(&app);
        assert_eq!(shorter.geometry.counts, [4, 2]);
        assert_eq!(shorter.geometry.endpoints(), initial.geometry.endpoints());
        assert_eq!(shorter.stagger, accumulated.stagger);
        check_position(&app, 7, [start[0] + 3.0 * 84.0 + 50.0, start[1] + 140.0 - 20.0]);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert_eq!(get(&app), shorter);
        let preview = app.s.doc.clone();
        app.array_panel.finish(&mut app.s, false);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert_eq!(get(&app), shorter);
        assert_eq!(app.s.doc, preview);
        app.array_panel.finish(&mut app.s, true);
        assert_eq!(app.s.doc, preview);
    }

    #[test]
    fn grid_real_start_end_and_box_drags_keep_history_and_switching_modes() {
        let (mut app, ctx) = linear_app();
        screenshot(&mut app, "array-grid-stagger");
        let selection = app.s.committed_document().0.clone();
        let current = |app: &App| app.array_panel.settings.as_ref().unwrap().grid;
        let drag = |app: &mut App, from, delta: [f32; 2], finish| {
            let pos = screen(app, from);
            let zoom = app.view.zoom;
            drag_gizmo(&ctx, app, pos, vec2(delta[0], delta[1]) * zoom, finish);
        };
        nodes(&ctx, &mut app);
        let original = current(&app);
        let ends = original.geometry.endpoints();
        drag(&mut app, ends.start, [24.0, 14.0], true);
        let resized = current(&app);
        near(resized.geometry.start, [184.0, 146.0]);
        near(resized.geometry.endpoints().end, ends.end);
        near(resized.geometry.spacing, [76.0, 63.0]);

        drag(&mut app, ends.end, [-354.0, -60.0], false);
        let crossed = current(&app);
        assert!(crossed.geometry.spacing[0] < 0.0 && crossed.geometry.spacing[1] > 0.0);
        near(crossed.geometry.start, resized.geometry.start);
        let first = app.s.selected_elements()[0].1.clone();
        assert!(first.transform.m[0] < 0.0 && first.transform.m[3] > 0.0);
        let release = screen(&app, crossed.geometry.endpoints().end);
        frame(&ctx, &mut app, vec![pointer_button(release, false)]);
        let crossed = current(&app);
        let [sx, sy] = crossed.geometry.start;
        let [ex, ey] = crossed.geometry.endpoints().end;
        drag(&mut app, [(sx + ex) * 0.5, (sy + ey) * 0.5], [24.0, 18.0], true);
        let moved = current(&app);
        near(moved.geometry.start, [sx + 24.0, sy + 18.0]);
        near(moved.geometry.endpoints().end, [ex + 24.0, ey + 18.0]);
        assert_eq!(moved.geometry.spacing, crossed.geometry.spacing);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(current(&app), crossed);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(current(&app), resized);
        history_key(&ctx, &mut app, Key::Z, Modifiers::SHIFT);
        assert_eq!(current(&app), crossed);

        let stop = app.array_panel.selected_stop;
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert_eq!(app.s.selected_elements().len(), 10);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert_eq!(current(&app), crossed);
        assert_eq!(app.array_panel.selected_stop, stop);
        assert_eq!(app.s.selected_elements().len(), 12);
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        assert!(!app.s.array_preview_active());
        assert_eq!(app.s.doc, selection);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Grid));
        assert_eq!(current(&app), crossed);
        let preview = app.s.doc.clone();
        let steps = app.s.history_len();
        app.array_panel.finish(&mut app.s, true);
        assert_eq!(app.s.doc, preview);
        assert_eq!(app.s.history_len(), steps + 1);
    }

    #[test]
    fn array_history_coalesces_stage_drags_and_commits_one_document_step() {
        use anicraft_engine::geom::Vec2 as Point;
        let (mut app, ctx) = linear_app();
        let (base, source, steps) = (app.s.doc.clone(), app.s.selection.clone(), app.s.history_len());
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        nodes(&ctx, &mut app);
        let initial = app.array_panel.settings.clone();
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings, initial);
        assert!(app.s.array_preview_active() && app.s.committed_document().0 == &base);

        let from = screen(&app, initial.as_ref().unwrap().linear.end);
        drag_gizmo(&ctx, &mut app, from, vec2(12.0, 8.0), false);
        frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(30.0, 20.0))]);
        frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(44.0, 26.0))]);
        frame(&ctx, &mut app, vec![pointer_button(from + vec2(44.0, 26.0), false)]);
        let linear = app.array_panel.settings.clone();
        assert_ne!(linear, initial);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings, initial);
        assert!(!app.array_panel.can_step(false));
        history_key(&ctx, &mut app, Key::Z, Modifiers::SHIFT);
        assert_eq!(app.array_panel.settings, linear);

        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert!(app.perform("edit.undo", json!({})).is_some());
        assert_eq!(app.array_panel.mode(), Some(Mode::Linear));
        assert_eq!(app.array_panel.settings, linear);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(app.array_panel.mode(), Some(Mode::Radial));
        let radial = Radial { centre: [260.0, 220.0], radius: 80.0, start: 0.0, sweep: 90.0,
            closed_loop: false, spiral_offset: 0.0, centre_drift: [70.0, -65.0], rotate: true };
        app.array_panel.set_gizmo_geometry(&mut app.s, Geometry::Radial(radial), None);
        nodes(&ctx, &mut app);
        let before = app.array_panel.settings.clone();
        let preview = app.s.doc.clone();
        let project = |app: &App, p: Point| screen(app, [p.x, p.y]);
        let from = project(&app, radial.point(1.0));
        let to = project(&app, Radial { sweep: 130.0, spiral_offset: 60.0, ..radial }.point(1.0));
        drag_gizmo(&ctx, &mut app, from, (to - from) * 0.5, false);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        let after = app.array_panel.settings.clone();
        let ended = after.as_ref().unwrap().radial;
        assert!((ended.sweep - 130.0).abs() < 0.01 && (ended.spiral_offset - 60.0).abs() < 0.05);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings, before);
        assert!(app.s.doc == preview && app.s.committed_document().0 == &base);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(app.array_panel.settings, after);
        let final_preview = app.s.doc.clone();
        let action = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &action);
        assert_eq!(app.s.history_len(), steps + 1);
        assert!(app.array_panel.settings.is_none() && !app.s.array_preview_active());
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert!(app.s.doc == base && app.s.selection == source);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert!(app.s.doc == final_preview);
    }

    #[test]
    fn array_history_coalesces_numeric_drags_and_typing_and_preserves_text_escape() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let shown = |app: &mut App, label: &str| nodes(&ctx, app).into_iter().find(|(_, n)| n.label() == Some(label)).unwrap();
        let initial = app.array_panel.settings.clone();
        let bounds = shown(&mut app, "Scale X").1.bounds().unwrap();
        let from = pos2(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
        drag_gizmo(&ctx, &mut app, from, vec2(20.0, 0.0), false);
        frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(46.0, 0.0))]);
        frame(&ctx, &mut app, vec![pointer_button(from + vec2(46.0, 0.0), false)]);
        let scaled = app.array_panel.settings.clone();
        assert_ne!(initial, scaled);
        assert_eq!(app.array_panel.active.as_ref().unwrap().history.undo.len(), 1);

        let edit = |app: &mut App| {
            let field = shown(app, "Rotation").1;
            click(&ctx, app, &field);
            frame(&ctx, app, vec![]);
        };
        let text = |app: &mut App, value: &str| frame(&ctx, app, vec![Event::Text(value.to_owned())]);
        edit(&mut app);
        text(&mut app, "7");
        text(&mut app, "2");
        assert_eq!(app.array_panel.active.as_ref().unwrap().history.undo.len(), 1);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert!(ctx.text_edit_focused());
        assert_eq!(app.array_panel.active.as_ref().unwrap().history.undo.len(), 1);
        frame(&ctx, &mut app, vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false,
            modifiers: Modifiers::COMMAND }, Event::Text("720".to_owned())]);
        frame(&ctx, &mut app, vec![key(Key::Enter, true)]);
        frame(&ctx, &mut app, vec![key(Key::Enter, false)]);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().stops[0].value.rotation, 720.0);
        assert_eq!(app.array_panel.active.as_ref().unwrap().history.undo.len(), 2);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings, scaled);

        edit(&mut app);
        text(&mut app, "97");
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        assert_eq!(app.array_panel.settings, scaled);
        assert!(app.array_panel.can_step(true) && app.s.array_preview_active());
        let alpha = shown(&mut app, "Alpha").0;
        set_number(&ctx, &mut app, alpha, 45.0);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().stops[0].value.alpha, 0.45);
        assert!(!app.array_panel.can_step(true));
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings, scaled);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings, initial);
    }

    #[test]
    fn array_history_restores_stop_creation_movement_properties_and_removal() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let node = |app: &mut App, label| nodes(&ctx, app).into_iter().find(|(_, n)| n.label() == Some(label)).unwrap();
        let settings = |app: &App| app.array_panel.settings.clone().unwrap();
        let initial = settings(&app);
        for label in ["Dim Stage", "Hide Array Handles"] {
            let control = node(&mut app, label).1;
            click(&ctx, &mut app, &control);
        }
        assert!(!app.array_panel.can_step(false));
        let track = node(&mut app, "Array: add a property stop").1;
        click(&ctx, &mut app, &track);
        let added = settings(&app);
        assert_eq!(added.stops.len(), 3);
        let added_marker = Some(added.stops[1].marker_id());
        assert_eq!(app.array_panel.selected_stop, added_marker);
        let stop = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label().is_some_and(|name| name.starts_with("Array stop 2 of"))).unwrap().1;
        let rect = stop.bounds().unwrap();
        let from = pos2(((rect.x0 + rect.x1) / 2.0) as f32, ((rect.y0 + rect.y1) / 2.0) as f32);
        drag_gizmo(&ctx, &mut app, from, vec2(20.0, 0.0), false);
        frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(40.0, 0.0))]);
        frame(&ctx, &mut app, vec![pointer_button(from + vec2(40.0, 0.0), false)]);
        let moved = settings(&app);
        assert_ne!(moved, added);
        let alpha = node(&mut app, "Alpha").0;
        set_number(&ctx, &mut app, alpha, 30.0);
        let coloured = settings(&app);
        let scale_y = node(&mut app, "Scale Y").0;
        set_number(&ctx, &mut app, scale_y, 145.0);
        let edited = settings(&app);
        assert_eq!(edited.stops[1].value.alpha, 0.30);
        assert_eq!(edited.stops[1].value.scale[1], 1.45);
        let remove = node(&mut app, "Remove array stop").1;
        click(&ctx, &mut app, &remove);
        assert_eq!(settings(&app).stops.len(), 2);

        for target in [edited.clone(), coloured, moved, added, initial] {
            history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
            assert_eq!(settings(&app), target);
            if target.stops.len() == 3 { assert_eq!(app.array_panel.selected_stop, added_marker); }
        }
        assert!(!app.array_panel.can_step(false));
        for _ in 0..4 { history_key(&ctx, &mut app, Key::Y, Modifiers::NONE); }
        assert_eq!(settings(&app), edited);
        assert!(!app.array_panel.dim_stage && !app.array_panel.show_gizmo);
        let copies = app.s.selected_elements();
        assert!(copies.iter().any(|(_, element)| element.color.alpha() < 0.8));
        assert!(copies.iter().any(|(_, element)| element.transform.m[3] > 1.2));
    }

    #[test]
    fn blur_fields_inherit_stop_values_and_keep_array_session_history() {
        let (mut app, ctx) = linear_app();
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": [
            {"type": "blur", "blur_x": 12, "blur_y": 4, "quality": 2},
            {"type": "adjust_color", "contrast": 20, "enabled": false},
        ]})).unwrap();
        app.s.execute("element.set_color_effect", &json!({"red_multiplier": 90, "alpha_multiplier": 80, "alpha_offset": 7})).unwrap();
        let source_element = source_element(&app.s).unwrap();
        let seeded = ArrayPropertyStop::from_source(&source_element);
        let source = app.s.doc.clone();
        let history = app.s.history_len();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let node = |app: &mut App, role: Option<Role>, name: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| role.is_none_or(|role| n.role() == role) && n.label() == Some(name))
            .unwrap_or_else(|| panic!("missing {name}"));
        let state = |app: &App| app.array_panel.settings.as_ref().unwrap().clone();
        let first_blur = |app: &App| {
            let copies = app.s.selected_elements();
            copies[0].1.kind.filters().unwrap()[0].filter.clone()
        };
        for name in ["Transform", "Colour Effect", "Blur 1"] {
            let header = node(&mut app, None, name).1;
            click(&ctx, &mut app, &header);
        }
        let initial = state(&app);
        assert_eq!(initial.stops[0].value, seeded);
        assert_eq!(app.s.selected_elements()[0].1.color, source_element.color);
        assert_eq!(initial.stops[0].value.filters, initial.stops[1].value.filters);
        let x = node(&mut app, Some(Role::SpinButton), "Blur 1 blur X").1;
        assert_eq!(x.numeric_value(), Some(12.0));
        let bounds = x.bounds().unwrap();
        let from = pos2(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
        drag_gizmo(&ctx, &mut app, from, vec2(16.0, 0.0), false);
        assert!(first_blur(&app).param(anicraft_engine::doc::FilterParam::BlurX) > Some(12.0));
        let to = from + vec2(32.0, 0.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        let dragged = state(&app);
        assert_eq!(app.array_panel.active.as_ref().unwrap().history.undo.len(), 1);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(state(&app), initial);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(state(&app), dragged);
        let y = node(&mut app, Some(Role::SpinButton), "Blur 1 blur Y").0;
        set_number(&ctx, &mut app, y, 40.0);
        assert_eq!(first_blur(&app).param(anicraft_engine::doc::FilterParam::BlurY), Some(40.0));
        let left = &mut app.array_panel.settings.as_mut().unwrap().stops[0].value;
        left.scale = [0.5, 1.75];
        left.rotation = 52.0;
        left.alpha = 0.625;
        left.color = ColorEffect::Tint { color: Rgba::hex(0x4477ee), amount: 0.75 };
        left.outbound = Some(Easing::QuadOut);
        app.array_panel.refresh(&mut app.s);
        let before_insert = state(&app);
        let track = node(&mut app, None, "Array: add a property stop").1;
        click(&ctx, &mut app, &track);
        let draft = state(&app);
        assert_eq!(draft.stops.len(), 3);
        let inserted = &draft.stops[1].value;
        assert_eq!(*inserted, ArrayPropertyStop { offset: inserted.offset, ..before_insert.stops[0].value.clone() });
        assert_eq!(app.array_panel.selected_stop, Some(draft.stops[1].marker_id()));
        assert_ne!(inserted.filters, initial.stops[0].value.filters);
        let easing = node(&mut app, None, "Easing").1;
        click(&ctx, &mut app, &easing);
        let filters = node(&mut app, Some(Role::CheckBox), "Filters").1;
        assert!(!filters.is_disabled());
        click(&ctx, &mut app, &filters);
        assert!(!state(&app).easing.filters);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(state(&app), draft);

        app.array_panel.finish(&mut app.s, false);
        assert_eq!(app.s.doc, source);
        assert_eq!(app.s.history_len(), history);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert_eq!(state(&app), draft);
        app.array_panel.reset(&mut app.s);
        assert_eq!(state(&app), initial);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(state(&app), draft);
        app.array_panel.finish(&mut app.s, true);
        assert_eq!(app.s.history_len(), history + 1);
        app.s.undo().unwrap();
        assert_eq!(app.s.doc, source);
    }

    #[test]
    fn shadow_glow_controls_group_gestures_and_inherit_the_left_stop() {
        use anicraft_engine::doc::{FilterColor, FilterParam};
        let (mut app, ctx) = linear_app();
        app.dock.right.size = Some(450.0);
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": [
            {"type": "drop_shadow", "distance": 12, "angle": 350, "color": "#2e3442"},
            {"type": "glow", "strength": 2, "color": "#4898cc"},
        ]})).unwrap();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let node = |app: &mut App, name: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label() == Some(name)).unwrap_or_else(|| panic!("missing {name}"));
        let state = |app: &App| app.array_panel.settings.as_ref().unwrap().clone();
        let steps = |app: &App| app.array_panel.active.as_ref().unwrap().history.undo.len();
        for label in ["Transform", "Colour Effect", "Drop Shadow 1"] {
            let header = node(&mut app, label).1;
            click(&ctx, &mut app, &header);
        }
        let to_edit = node(&mut app, "Drop Shadow 1 strength").1;
        let b = to_edit.bounds().unwrap();
        let from = pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
        let before = steps(&app);
        begin_drag(&ctx, &mut app, from);
        for dx in [9.0, 18.0] { frame(&ctx, &mut app, vec![Event::PointerMoved(from + vec2(dx, 0.0))]); }
        let strength = state(&app).stops[0].value.filters[0].param(FilterParam::Strength);
        assert!(strength > Some(100.0), "strength {strength:?} after drag from {from:?}, {:?}", ctx.dragged_id());
        let to = from + vec2(32.0, 0.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        assert_eq!(steps(&app), before + 1);
        for (label, value) in [("Drop Shadow 1 angle", 10.0), ("Drop Shadow 1 distance", 24.0)] {
            let field = node(&mut app, label).0;
            set_number(&ctx, &mut app, field, value);
        }
        for label in ["Drop Shadow 1 quality High", "Drop Shadow 1 Inner", "Drop Shadow 1 Knockout", "Drop Shadow 1 Hide object"] {
            let control = node(&mut app, label).1;
            click(&ctx, &mut app, &control);
        }
        let swatch = node(&mut app, "Drop Shadow 1 colour").1;
        let before = steps(&app);
        click(&ctx, &mut app, &swatch);
        let hex = node(&mut app, "Hex colour").1;
        click(&ctx, &mut app, &hex);
        for (typed, rgb) in [("#ff4200", 0xff4200), ("#10aabb", 0x10aabb)] {
            history_key(&ctx, &mut app, Key::A, Modifiers::NONE);
            frame(&ctx, &mut app, vec![Event::Text(typed.to_owned())]);
            assert_eq!(state(&app).stops[0].value.filters[0].color(FilterColor::Color), Some(Rgba::hex(rgb)));
        }
        assert_eq!(steps(&app), before);
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        assert_eq!(steps(&app), before + 1);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(state(&app).stops[0].value.filters[0].color(FilterColor::Color), Some(Rgba::hex(0x2e3442)));
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        let shadow = &state(&app).stops[0].value.filters[0];
        assert_eq!(shadow.param(FilterParam::Angle), Some(10.0));
        assert_eq!(shadow.param(FilterParam::Distance), Some(24.0));
        assert_eq!(FilterParam::STEPPED.map(|p| shadow.param(p)), [Some(3.0), Some(1.0), Some(1.0), Some(1.0)]);
        let header = node(&mut app, "Drop Shadow 1").1;
        click(&ctx, &mut app, &header);
        if !nodes(&ctx, &mut app).iter().any(|(_, n)| n.label() == Some("Glow 2 strength")) {
            let glow = node(&mut app, "Glow 2").1;
            click(&ctx, &mut app, &glow);
        }
        let strength = node(&mut app, "Glow 2 strength").0;
        set_number(&ctx, &mut app, strength, 340.0);
        let inner = node(&mut app, "Glow 2 Inner").1;
        click(&ctx, &mut app, &inner);
        let left = &mut app.array_panel.settings.as_mut().unwrap().stops[0].value;
        left.outbound = Some(Easing::QuadOut);
        app.array_panel.refresh(&mut app.s);
        let authored = state(&app);
        assert_eq!(authored.stops[0].value.filters[1].param(FilterParam::Strength), Some(340.0));
        assert_eq!(authored.stops[0].value.filters[1].param(FilterParam::Inner), Some(1.0));
        assert_eq!(authored.stops[1].value.filters[1].param(FilterParam::Strength), Some(200.0));
        let track = node(&mut app, "Array: add a property stop").1;
        click(&ctx, &mut app, &track);
        let inserted = state(&app);
        let stop = &inserted.stops[1];
        assert_eq!(stop.value, ArrayPropertyStop { offset: stop.value.offset, ..authored.stops[0].value.clone() });
        assert_eq!(app.array_panel.selected_stop, Some(stop.marker_id()));
        let remove = node(&mut app, "Remove array stop").1;
        click(&ctx, &mut app, &remove);
        assert_eq!(state(&app), authored);
        assert!(app.array_panel.selected_stop.is_some());
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(state(&app), inserted);
        assert_eq!(app.array_panel.selected_stop, Some(stop.marker_id()));
    }

    #[test]
    fn linear_array_adjust_color_card_edits_and_inherits_independent_stops() {
        let (mut app, ctx) = linear_app();
        app.dock.right.size = Some(450.0);
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": [
            {"type": "adjust_color", "brightness": 5, "contrast": -10, "saturation": 22, "hue": -90},
            {"type": "adjust_color", "brightness": 13, "hue": 70, "enabled": false},
        ]})).unwrap();
        let source = app.s.doc.clone();
        let doc_steps = app.s.history_len();
        let state = |app: &App| app.array_panel.settings.clone().unwrap();
        let steps = |app: &App| app.array_panel.active.as_ref().unwrap().history.undo.len();
        let pixels = |app: &App| anicraft_render::render_frame(&app.s.doc, 0, 1.0).unwrap().to_rgba8();
        let find = |app: &mut App, label: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label() == Some(label)).unwrap_or_else(|| panic!("missing {label}"));
        let press = |app: &mut App, label: &str| {
            let (_, node) = find(app, label);
            click(&ctx, app, &node);
        };
        let set = |app: &mut App, name: &str, value| {
            let (id, control) = find(app, name);
            assert_eq!(control.role(), Role::SpinButton);
            set_number(&ctx, app, id, value);
        };

        press(&mut app, "Array type");
        press(&mut app, "Linear");
        for label in ["Transform", "Colour Effect", "Adjust Color 2 (off)"] { press(&mut app, label); }
        assert!(nodes(&ctx, &mut app).iter().all(|(_, n)| n.label() != Some("Adjust Color 1 Hue")));
        assert!(find(&mut app, "Adjust Color 2 Hue").1.is_disabled());
        press(&mut app, "Adjust Color 2 (off)");
        press(&mut app, "Adjust Color 1");
        for parameter in ["Brightness", "Contrast", "Saturation", "Hue"] {
            let (_, control) = find(&mut app, &format!("Adjust Color 1 {parameter}"));
            assert!(!control.is_disabled());
            assert_eq!(control.role(), Role::SpinButton);
        }
        let initial = state(&app);
        assert_eq!(initial.stops[0].value.filters, initial.source_defaults.filters);
        assert_eq!(initial.stops[1].value.filters, initial.source_defaults.filters);
        let baseline = pixels(&app);
        let b = find(&mut app, "Adjust Color 1 Brightness").1.bounds().unwrap();
        let from = pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
        let before_drag = steps(&app);
        drag_gizmo(&ctx, &mut app, from, vec2(18.0, 0.0), false);
        let to = from + vec2(36.0, 0.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        assert_eq!(steps(&app), before_drag + 1);
        assert_ne!(state(&app).stops[0].value.filters[0], initial.stops[0].value.filters[0]);
        set(&mut app, "Adjust Color 1 Contrast", 31.0);
        set(&mut app, "Adjust Color 1 Saturation", 130.0);
        press(&mut app, "Adjust Color 1 Hue");
        frame(&ctx, &mut app, vec![]);
        let before_typing = steps(&app);
        frame(&ctx, &mut app, vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false,
            modifiers: Modifiers::COMMAND }, Event::Text("-180".to_owned())]);
        frame(&ctx, &mut app, vec![key(Key::Enter, true)]);
        frame(&ctx, &mut app, vec![key(Key::Enter, false)]);
        assert_eq!(steps(&app), before_typing + 1);
        let left = &mut app.array_panel.settings.as_mut().unwrap().stops[0].value;
        assert!(matches!(left.filters[0], Filter::AdjustColor { contrast: 31.0, saturation: 100.0, hue: -180.0, .. }));
        left.outbound = Some(Easing::QuadIn);
        app.array_panel.refresh(&mut app.s);
        let first_pixels = pixels(&app);
        assert_ne!(first_pixels, baseline);
        let (_, last) = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label().is_some_and(|label| label.starts_with("Array stop 2 of"))).unwrap();
        click(&ctx, &mut app, &last);
        assert_eq!(state(&app).stops[1].value.filters, initial.stops[1].value.filters);
        set(&mut app, "Adjust Color 1 Hue", 180.0);
        let preview = pixels(&app);
        assert_ne!(preview, first_pixels);
        let authored = state(&app);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(pixels(&app), first_pixels);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(state(&app), authored);
        press(&mut app, "Array: add a property stop");
        let inserted = state(&app);
        let stop = &inserted.stops[1];
        assert_eq!(stop.value, ArrayPropertyStop { offset: stop.value.offset, ..authored.stops[0].value.clone() });
        press(&mut app, "Remove array stop");
        assert_eq!(state(&app), authored);
        let action = commit_control(&ctx, &mut app, "Cancel");
        click(&ctx, &mut app, &action);
        assert_eq!(app.s.doc, source);
        assert_eq!(app.s.history_len(), doc_steps);
        press(&mut app, "Array type");
        press(&mut app, "Linear");
        assert_eq!(state(&app), authored);
        assert_eq!(pixels(&app), preview);
        let action = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &action);
        assert_eq!(pixels(&app), preview);
        assert_eq!(app.s.history_len(), doc_steps + 1);
    }

    #[test]
    fn linear_array_gradient_glow_card_edits_corresponding_ramp_stops() {
        edit_array_gradient_card("gradient_glow", "Gradient Glow");
    }

    #[test]
    fn linear_array_gradient_bevel_card_edits_corresponding_ramp_stops() {
        edit_array_gradient_card("gradient_bevel", "Gradient Bevel");
    }

    fn edit_array_gradient_card(kind: &str, title: &str) {
        use anicraft_engine::doc::GradientStop;
        let (mut app, ctx) = linear_app();
        app.dock.right.size = Some(450.0);
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        let bevel = kind == "gradient_bevel";
        let mut stops = vec![
            json!({"offset": 0, "color": "#ffffff00"}), json!({"offset": 0.25, "color": "#7090ffaa"}),
            json!({"offset": 0.75, "color": "#d0db77ee"}), json!({"offset": 1, "color": "#300000ff"}),
        ];
        if bevel { stops.insert(2, json!({"offset": 0.5, "color": "#00113300"})); }
        app.s.execute("element.set_filters", &json!({"filters": [{"type": kind, "stops": stops}]})).unwrap();
        let source = app.s.doc.clone();
        let doc_steps = app.s.history_len();
        let find = |app: &mut App, label: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label() == Some(label)).unwrap_or_else(|| panic!("missing {label}"));
        let press = |app: &mut App, label: &str| {
            let (_, node) = find(app, label);
            click(&ctx, app, &node);
        };
        let find_prefix = |app: &mut App, prefix: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label().is_some_and(|label| label.starts_with(prefix))).unwrap_or_else(|| panic!("missing {prefix}"));
        let settings = |app: &App| app.array_panel.settings.as_ref().unwrap().clone();
        let ramp = |app: &App, slot: usize| -> Vec<GradientStop> {
            let stop = &app.array_panel.settings.as_ref().unwrap().stops[slot];
            let (Filter::GradientGlow { stops, .. } | Filter::GradientBevel { stops, .. }) = &stop.value.filters[0] else { panic!() };
            stops.clone()
        };
        let steps = |app: &App| app.array_panel.active.as_ref().unwrap().history.undo.len();
        let render = |app: &App| anicraft_render::render_frame(&app.s.doc, 0, 1.0).unwrap().data;
        let set_offset = |app: &mut App, pct| {
            let field = find(app, &format!("{title} stop position")).0;
            set_number(&ctx, app, field, pct);
        };
        let select_array_stop = |app: &mut App, prefix| {
            let (_, node) = find_prefix(app, prefix);
            click(&ctx, app, &node);
        };

        press(&mut app, "Array type");
        press(&mut app, "Linear");
        for label in ["Transform", "Colour Effect", title] { press(&mut app, label); }
        assert!(app.s.array_preview_active());
        let controls = nodes(&ctx, &mut app);
        for control in ["blur X", "blur Y", "strength", "angle", "distance", "stop colour", "stop position", "quality High",
            "type Inner", "type Outer", "type Full", "knockout"] {
            let label = format!("{title} {control}");
            assert!(controls.iter().any(|(_, n)| n.label() == Some(label.as_str()) && !n.is_disabled()), "missing enabled {label}");
        }
        for label in ["Uses source settings".into(), format!("{title} gradient: add a colour stop"), format!("Remove {title} colour stop")] {
            assert!(controls.iter().all(|(_, n)| n.label() != Some(label.as_str())), "unexpected {label}");
        }
        let (pin_index, pin_description, pin) = if bevel { (2, "3 at 50%", 0.5) } else { (0, "1 at 0%", 0.0) };
        press(&mut app, &format!("{title} colour stop {pin_description} (fixed)"));
        assert!(find(&mut app, &format!("{title} stop position")).1.is_disabled());
        assert_eq!((ramp(&app, 0)[pin_index].offset, ramp(&app, 0)[pin_index].color.a), (pin, 0.0));
        let marker = format!("{title} colour stop 2 at 25%");
        press(&mut app, &marker);
        let b = find(&mut app, &marker).1.bounds().unwrap();
        let from = pos2(((b.x0 + b.x1) * 0.5) as f32, ((b.y0 + b.y1) * 0.5) as f32);
        let history = steps(&app);
        let baseline = render(&app);
        begin_drag(&ctx, &mut app, from);
        let to = from + vec2(40.0, 0.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        assert!(ramp(&app, 0)[1].offset > 0.25);
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        assert_eq!(steps(&app), history + 1);
        set_offset(&mut app, 90.0);
        assert_eq!(ramp(&app, 0)[1].offset, if bevel { 0.498 } else { 0.75 }, "typing beyond the next stop keeps the ramp ordered");
        set_offset(&mut app, 40.0);

        let edit_colour = |app: &mut App, typed, rgba: [u8; 4], slot| {
            let before = settings(app);
            let history = steps(app);
            press(app, &format!("{title} stop colour"));
            press(app, "Hex colour");
            for value in ["#aabbcc20", typed] {
                history_key(&ctx, app, Key::A, Modifiers::NONE);
                frame(&ctx, app, vec![Event::Text(value.to_owned())]);
            }
            let [r, g, b, a] = rgba.map(|v| f32::from(v) / 255.0);
            assert_eq!(ramp(app, slot)[1].color, Rgba::new(r, g, b, a));
            assert_eq!(steps(app), history);
            let edited = settings(app);
            frame(&ctx, app, vec![key(Key::Escape, true)]);
            frame(&ctx, app, vec![key(Key::Escape, false)]);
            assert_eq!(steps(app), history + 1);
            history_key(&ctx, app, Key::Z, Modifiers::NONE);
            assert_eq!(settings(app), before);
            history_key(&ctx, app, Key::Y, Modifiers::NONE);
            assert_eq!(settings(app), edited);
        };
        edit_colour(&mut app, "#e9344a70", [233, 52, 74, 112], 0);
        let left = ramp(&app, 0);
        select_array_stop(&mut app, "Array stop 2 of");
        assert_eq!(ramp(&app, 1)[1].offset, 0.25);
        if bevel {
            press(&mut app, &format!("{title} colour stop 4 at 75%"));
            set_offset(&mut app, 10.0);
            assert_eq!(ramp(&app, 1)[3].offset, 0.502);
            assert_eq!((ramp(&app, 1)[2].offset, ramp(&app, 1)[2].color.a), (0.5, 0.0));
        }
        press(&mut app, &marker);
        set_offset(&mut app, if bevel { 45.0 } else { 60.0 });
        edit_colour(&mut app, "#11bbd6c0", [17, 187, 214, 192], 1);
        assert_eq!(ramp(&app, 0), left);
        assert_eq!(ramp(&app, 1)[1].offset, if bevel { 0.45 } else { 0.6 });
        let preview = render(&app);
        assert_ne!(preview, baseline);
        let authored = settings(&app);
        press(&mut app, "Array: add a property stop");
        let inserted = settings(&app);
        assert_eq!(inserted.stops[1].value, ArrayPropertyStop { offset: inserted.stops[1].value.offset, ..authored.stops[0].value.clone() });
        assert_eq!(ramp(&app, 1), left);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(settings(&app), authored);
        let action = commit_control(&ctx, &mut app, "Cancel");
        click(&ctx, &mut app, &action);
        assert_eq!(app.s.doc, source);
        assert_eq!(app.s.history_len(), doc_steps);
        press(&mut app, "Array type");
        press(&mut app, "Linear");
        assert_eq!(settings(&app), authored);
        assert_eq!(render(&app), preview);
        let action = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &action);
        assert_eq!(render(&app), preview);
        assert_eq!(app.s.history_len(), doc_steps + 1);
    }

    #[test]
    fn linear_array_opens_the_source_bevel_editor_and_commits_the_edited_preview() {
        use anicraft_engine::doc::FilterParam;
        let (mut app, ctx) = linear_app();
        app.dock.right.size = Some(450.0);
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": [{"type": "bevel"}]})).unwrap();
        let source = app.s.doc.clone();
        let doc_steps = app.s.history_len();
        let find = |app: &mut App, label: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label() == Some(label)).unwrap_or_else(|| panic!("missing {label}"));
        let press = |app: &mut App, label| {
            let (_, node) = find(app, label);
            click(&ctx, app, &node);
        };
        let marker = |app: &mut App, prefix: &str| {
            let (_, node) = nodes(&ctx, app).into_iter().find(|(_, n)| n.label().is_some_and(|label| label.starts_with(prefix))).unwrap();
            click(&ctx, app, &node);
        };
        let pixels = |app: &App| anicraft_engine::render::render_frame(&app.s.doc, 0, 1.0).unwrap().data;

        press(&mut app, "Array type");
        press(&mut app, "Linear");
        assert!(app.s.array_preview_active());
        for label in ["Transform", "Colour Effect", "Filters", "Filters"] { press(&mut app, label); }
        assert!(nodes(&ctx, &mut app).iter().all(|(_, n)| n.label() != Some("Bevel distance")), "the closed Bevel card hides its controls");
        press(&mut app, "Bevel");
        let controls = nodes(&ctx, &mut app);
        for label in ["Bevel blur X", "Bevel blur Y", "Bevel strength", "Bevel angle", "Bevel distance",
            "Bevel shadow colour", "Bevel highlight colour", "Bevel quality Low", "Bevel quality Medium", "Bevel quality High",
            "Bevel type Inner", "Bevel type Outer", "Bevel type Full", "Bevel knockout"] {
            assert!(controls.iter().any(|(_, n)| n.label() == Some(label) && !n.is_disabled()), "missing enabled {label}");
        }
        assert!(controls.iter().all(|(_, n)| n.label() != Some("Uses source settings") && n.value() != Some("Uses source settings")));

        let baseline = pixels(&app);
        let distance = find(&mut app, "Bevel distance").0;
        set_number(&ctx, &mut app, distance, 30.0);
        let first_edit = pixels(&app);
        assert_ne!(first_edit, baseline, "editing the first stop updates the real preview pixels");
        marker(&mut app, "Array stop 2 of");
        let distance = find(&mut app, "Bevel distance").0;
        set_number(&ctx, &mut app, distance, 12.0);
        let preview = pixels(&app);
        assert_ne!(preview, first_edit, "the other stop edits the preview independently");
        let distances: Vec<_> = app.s.selected_elements().iter().map(|(_, e)| {
            e.kind.filters().unwrap()[0].filter.param(FilterParam::Distance).unwrap()
        }).collect();
        assert_eq!(distances.first(), Some(&30.0));
        assert_eq!(distances.last(), Some(&12.0));
        assert!(distances.windows(2).all(|pair| pair[0] > pair[1]), "{distances:?}");
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(pixels(&app), first_edit);
        assert!(app.s.array_preview_active());
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(pixels(&app), preview);
        let action = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &action);
        assert!(!app.s.array_preview_active());
        assert_eq!(app.s.history_len(), doc_steps + 1);
        assert_eq!(pixels(&app), preview);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.s.doc, source);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(pixels(&app), preview);
    }

    #[test]
    fn bevel_controls_keep_rgba_picker_gestures_and_left_stop_inheritance() {
        use anicraft_engine::doc::{FilterColor, FilterParam, FilterSide};
        let (mut app, ctx) = linear_app();
        app.dock.right.size = Some(450.0);
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": [{"type": "bevel"}]})).unwrap();
        let original = app.s.doc.clone();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let node = |app: &mut App, name: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label() == Some(name)).unwrap_or_else(|| panic!("missing {name}"));
        let state = |app: &App| app.array_panel.settings.as_ref().unwrap().clone();
        let steps = |app: &App| app.array_panel.active.as_ref().unwrap().history.undo.len();
        for label in ["Transform", "Colour Effect", "Bevel"] {
            let header = node(&mut app, label).1;
            click(&ctx, &mut app, &header);
        }
        let strength = node(&mut app, "Bevel strength").1.bounds().unwrap();
        let from = pos2(((strength.x0 + strength.x1) / 2.0) as f32, ((strength.y0 + strength.y1) / 2.0) as f32);
        let history = steps(&app);
        begin_drag(&ctx, &mut app, from);
        let to = from + vec2(24.0, 0.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        assert!(state(&app).stops[0].value.filters[0].param(FilterParam::Strength) > Some(100.0));
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        assert_eq!(steps(&app), history + 1);
        for (label, param, value) in [
            ("Bevel blur X", FilterParam::BlurX, 12.0), ("Bevel blur Y", FilterParam::BlurY, 19.0),
            ("Bevel strength", FilterParam::Strength, 220.0), ("Bevel angle", FilterParam::Angle, 350.0),
            ("Bevel distance", FilterParam::Distance, -9.0),
        ] {
            let field = node(&mut app, label).0;
            set_number(&ctx, &mut app, field, value);
            assert_eq!(state(&app).stops[0].value.filters[0].param(param), Some(value as f32));
        }
        for label in ["Bevel quality High", "Bevel type Outer", "Bevel type Full", "Bevel knockout"] {
            let target_node = node(&mut app, label).0;
            frame(&ctx, &mut app, vec![Event::AccessKitActionRequest(ActionRequest {
                action: Action::Click, target_tree: TreeId::ROOT, target_node, data: None,
            })]);
        }
        let filter = state(&app).stops[0].value.filters[0].clone();
        assert!(matches!(filter, Filter::Bevel { quality: 3, side: FilterSide::Full, knockout: true, .. }), "{filter:?}");

        for (label, colour, final_hex, channels) in [
            ("Bevel shadow colour", FilterColor::Shadow, "#195ab680", [25, 90, 182, 128]),
            ("Bevel highlight colour", FilterColor::Highlight, "#fefdbea0", [254, 253, 190, 160]),
        ] {
            let before = state(&app);
            let history = steps(&app);
            let swatch = node(&mut app, label).1;
            click(&ctx, &mut app, &swatch);
            let hex = node(&mut app, "Hex colour").1;
            click(&ctx, &mut app, &hex);
            for typed in ["#aabbcc20", final_hex] {
                history_key(&ctx, &mut app, Key::A, Modifiers::NONE);
                frame(&ctx, &mut app, vec![Event::Text(typed.to_owned())]);
            }
            let [r, g, b, a] = channels.map(|v| v as f32 / 255.0);
            let edited = state(&app);
            assert_eq!(edited.stops[0].value.filters[0].color(colour), Some(Rgba::new(r, g, b, a)));
            assert_eq!(steps(&app), history);
            frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
            frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
            assert_eq!(steps(&app), history + 1);
            history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
            assert_eq!(state(&app), before);
            history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
            assert_eq!(state(&app), edited);
        }
        let left = &mut app.array_panel.settings.as_mut().unwrap().stops[0].value;
        left.outbound = Some(Easing::QuadOut);
        app.array_panel.refresh(&mut app.s);
        let authored = state(&app);
        let track = node(&mut app, "Array: add a property stop").1;
        click(&ctx, &mut app, &track);
        let inserted = state(&app);
        let stop = &inserted.stops[1];
        assert_eq!(stop.value, ArrayPropertyStop { offset: stop.value.offset, ..authored.stops[0].value.clone() });
        assert_eq!(app.array_panel.selected_stop, Some(stop.marker_id()));
        assert_eq!(inserted.stops[2].value, authored.stops[1].value);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(state(&app), authored);
        app.array_panel.finish(&mut app.s, false);
        assert_eq!(app.s.doc, original);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert_eq!(state(&app).stops, authored.stops);
    }

    #[test]
    fn stop_colour_styles_share_alpha_and_picker_changes_are_one_live_undo_step() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let node = |app: &mut App, label: &str| {
            nodes(&ctx, app).into_iter().find(|(_, n)| n.label() == Some(label))
                .unwrap_or_else(|| panic!("missing Array control {label}"))
        };
        let numeric = |app: &mut App, label| {
            nodes(&ctx, app).into_iter().find(|(_, n)| n.role() == Role::SpinButton && n.label() == Some(label))
                .unwrap_or_else(|| panic!("missing Array numeric field {label}"))
        };
        let number = |app: &mut App, label, value| {
            let (id, _) = numeric(app, label);
            set_number(&ctx, app, id, value);
        };
        let style = |app: &mut App, name| {
            let (_, chooser) = node(app, "Colour effect style");
            assert_eq!(chooser.role(), Role::ComboBox);
            click(&ctx, app, &chooser);
            let option = node(app, name).1;
            click(&ctx, app, &option);
        };
        let stop = |app: &App| app.array_panel.settings.as_ref().unwrap().stops[0].value.clone();
        let first_effect = |app: &App| app.s.selected_elements()[0].1.color;
        let steps = |app: &App| app.array_panel.active.as_ref().unwrap().history.undo.len();
        let original = app.s.committed_document().0.clone();

        number(&mut app, "Alpha", 75.0);
        style(&mut app, "Brightness");
        number(&mut app, "Brightness", -45.0);
        assert_eq!(stop(&app).color, ColorEffect::Brightness(-0.45));
        assert_eq!(first_effect(&app), ColorEffect::Brightness(-0.45).with_alpha(0.75));
        let bounds = numeric(&mut app, "Brightness").1.bounds().unwrap();
        let from = pos2(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
        let before_drag = steps(&app);
        drag_gizmo(&ctx, &mut app, from, vec2(12.0, 0.0), false);
        assert_ne!(first_effect(&app), ColorEffect::Brightness(-0.45).with_alpha(0.75));
        let to = from + vec2(24.0, 0.0);
        frame(&ctx, &mut app, vec![Event::PointerMoved(to)]);
        frame(&ctx, &mut app, vec![pointer_button(to, false)]);
        assert_eq!(steps(&app), before_drag + 1);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        style(&mut app, "Advanced");
        assert!(!nodes(&ctx, &mut app).iter().any(|(_, n)| n.role() == Role::SpinButton && n.label() == Some("Alpha")));
        assert_eq!(node(&mut app, "Alpha multiplier").1.numeric_value(), Some(75.0));
        for (label, value) in [("Alpha multiplier", 50.0), ("Alpha offset", 64.0), ("Red multiplier", -60.0), ("Red offset", 75.0),
            ("Green multiplier", 20.0), ("Green offset", -4.0), ("Blue multiplier", 80.0), ("Blue offset", 12.0)]
        {
            number(&mut app, label, value);
        }
        let advanced = first_effect(&app);
        assert_eq!(advanced.transform().mul, [-0.6, 0.2, 0.8, 0.5]);
        assert_eq!(advanced.transform().add, [75.0, -4.0, 12.0, 64.0].map(|v| v / 255.0));
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(first_effect(&app).transform().add[2], 0.0);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(first_effect(&app), advanced);
        style(&mut app, "Tint");
        assert_eq!(numeric(&mut app, "Alpha").1.numeric_value(), Some(50.0));
        number(&mut app, "Tint amount", 80.0);
        let before = steps(&app);
        let swatch = node(&mut app, "Tint colour").1;
        click(&ctx, &mut app, &swatch);
        let hex = node(&mut app, "Hex colour").1;
        click(&ctx, &mut app, &hex);
        for (typed, rgb) in [("#ff4000", 0xff4000), ("#20abcd", 0x20abcd)] {
            history_key(&ctx, &mut app, Key::A, Modifiers::NONE);
            frame(&ctx, &mut app, vec![Event::Text(typed.to_owned())]);
            let tint = ColorEffect::Tint { color: Rgba::hex(rgb), amount: 0.8 };
            assert_eq!(stop(&app).color, tint);
            assert_eq!(first_effect(&app), tint.with_alpha(0.5));
        }
        let tint = stop(&app);
        assert_eq!(steps(&app), before);
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        assert_eq!(steps(&app), before + 1);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(stop(&app).color, ColorEffect::Tint { color: Rgba::WHITE, amount: 0.8 });
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(stop(&app), tint);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().stops[1].value, ArrayPropertyStop::new(1.0, 1.0));

        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        app.array_panel.finish(&mut app.s, false);
        assert!(app.s.doc == original);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(stop(&app), tint);
        let history = app.s.history_len();
        app.array_panel.finish(&mut app.s, true);
        assert_eq!(app.s.history_len(), history + 1);
        assert!(app.array_panel.settings.is_none());
    }

    #[test]
    fn easing_controls_preview_and_follow_session_history_and_draft_lifecycle() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let original = app.array_panel.settings.clone().unwrap();
        let steps = app.s.history_len();
        let node = |app: &mut App, label: &str| {
            nodes(&ctx, app).into_iter().find(|(_, node)| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing Array control {label}")).1
        };
        let choose = |app: &mut App, control, option| {
            let combo = nodes(&ctx, app).into_iter().find(|(_, node)| {
                node.role() == Role::ComboBox && node.label() == Some(control)
            }).unwrap().1;
            click(&ctx, app, &combo);
            let option = node(app, option);
            click(&ctx, app, &option);
            nodes(&ctx, app);
        };
        let sampled_x = |app: &App| app.s.selected_elements()[3].1.transform.m[4];
        let line_x = sampled_x(&app);
        let header = node(&mut app, "Easing");
        click(&ctx, &mut app, &header);
        assert!(node(&mut app, "Filters").is_disabled());
        choose(&mut app, "Ease", "Sine Out");
        assert_eq!(app.array_panel.settings.as_ref().unwrap().easing.preset, Easing::SineOut);
        assert!(sampled_x(&app) > line_x);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(sampled_x(&app), line_x);
        choose(&mut app, "Ease", "Back Out");
        assert!(sampled_x(&app) > line_x);
        choose(&mut app, "Easing mode", "Per Segment");
        assert_eq!(sampled_x(&app), line_x);
        choose(&mut app, "Ease", "Quad Out");
        let target = nodes(&ctx, &mut app).into_iter().find(|(_, n)| {
            n.role() == Role::CheckBox && n.label() == Some("Distribution")
        }).unwrap().1;
        click(&ctx, &mut app, &target);
        assert_eq!(sampled_x(&app), line_x);
        let draft = app.array_panel.settings.clone().unwrap();
        assert_eq!(draft.stops[0].value.outbound, Some(Easing::QuadOut));
        assert_eq!(draft.stops[1], original.stops[1]);
        assert_eq!(draft.easing, ArrayEasing {
            mode: ArrayEaseMode::PerSegment, preset: Easing::BackOut, distribution: false, ..ArrayEasing::default()
        });
        assert_eq!(app.array_panel.active.as_ref().unwrap().history.undo.len(), 4);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert!(sampled_x(&app) > line_x);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(app.array_panel.settings.as_ref(), Some(&draft));
        assert_eq!(sampled_x(&app), line_x);

        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert_eq!(app.array_panel.settings.as_ref(), Some(&draft));
        app.array_panel.select_mode(&mut app.s, None);
        assert_eq!(app.s.history_len(), steps);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(app.array_panel.settings.as_ref(), Some(&draft));
        assert!(!app.array_panel.can_step(false));
        app.array_panel.finish(&mut app.s, true);
        assert_eq!(app.s.history_len(), steps + 1);
        let baked = app.s.selection.ids().next().unwrap();
        app.s.select_only([baked]);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(app.array_panel.settings.as_ref().unwrap().easing, ArrayEasing::default());
        app.array_panel.finish(&mut app.s, false);
    }

    #[test]
    fn outbound_picker_follows_the_selected_stop_and_history() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(app.array_panel.selected_stop, Some(0));
        let named = |app: &mut App, label: &str| {
            nodes(&ctx, app).into_iter().find(|(_, n)| n.label() == Some(label))
                .unwrap_or_else(|| panic!("missing {label}")).1
        };
        let picker = |app: &mut App, label: &str| {
            nodes(&ctx, app).into_iter().find(|(_, n)| n.role() == Role::ComboBox && n.label() == Some(label)).unwrap().1
        };
        let choose = |app: &mut App, control, option| {
            let combo = picker(app, control);
            click(&ctx, app, &combo);
            let option = named(app, option);
            click(&ctx, app, &option);
        };
        let select = |app: &mut App, number| {
            let prefix = format!("Array stop {number} of");
            let stop = nodes(&ctx, app).into_iter().find(|(_, n)| n.label().is_some_and(|label| label.starts_with(&prefix))).unwrap().1;
            click(&ctx, app, &stop);
        };
        let check = |app: &mut App, value, disabled| {
            let picker = picker(app, "Ease");
            assert_eq!(picker.value(), Some(value));
            assert_eq!(picker.is_disabled(), disabled);
        };
        let header = named(&mut app, "Easing");
        click(&ctx, &mut app, &header);
        choose(&mut app, "Ease", "Back Out");
        choose(&mut app, "Easing mode", "Per Segment");
        check(&mut app, "Linear", false);
        choose(&mut app, "Ease", "Quad Out");

        let track = named(&mut app, "Array: add a property stop");
        click(&ctx, &mut app, &track);
        let inserted = app.array_panel.settings.clone().unwrap();
        let marker = inserted.stops[1].marker_id();
        assert_eq!(app.array_panel.selected_stop, Some(marker));
        assert_eq!(inserted.stops[1].value.outbound, Some(Easing::QuadOut));
        check(&mut app, "Quad Out", false);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().stops.len(), 2);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(app.array_panel.settings.as_ref(), Some(&inserted));
        choose(&mut app, "Ease", "Sine In");
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        check(&mut app, "Quad Out", false);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        check(&mut app, "Sine In", false);
        select(&mut app, 1);
        check(&mut app, "Quad Out", false);
        select(&mut app, 3);
        check(&mut app, "Linear", true);
        assert!(!named(&mut app, "Alpha").is_disabled());
        choose(&mut app, "Easing mode", "Global");
        check(&mut app, "Back Out", false);
        choose(&mut app, "Ease", "Bounce Out");
        choose(&mut app, "Easing mode", "Per Segment");
        check(&mut app, "Linear", true);
        select(&mut app, 2);
        check(&mut app, "Sine In", false);
        let remove = named(&mut app, "Remove array stop");
        click(&ctx, &mut app, &remove);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().stops[0].value.outbound, Some(Easing::QuadOut));
        check(&mut app, "Linear", true);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(app.array_panel.selected_stop, Some(marker));
        check(&mut app, "Sine In", false);
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        check(&mut app, "Linear", true);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().easing.preset, Easing::BounceOut);
    }

    #[test]
    fn quitting_resolves_array_before_the_normal_document_save_flow() {
        for (choice, dirty, apply, cancel_save) in [
            ("Keep Editing", false, false, false),
            ("Discard Array and Quit", false, false, false),
            ("Discard Array and Quit", true, false, true),
            ("Apply Array and Quit", false, true, true),
            ("Apply Array and Quit", false, true, false),
        ] {
            let (mut app, ctx) = linear_app();
            if !dirty { app.s.mark_saved(); }
            let (original, steps) = (app.s.doc.clone(), app.s.history_len());
            app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
            let mut ends = app.array_panel.gizmo_endpoints().unwrap();
            ends.end[0] += 40.0;
            app.array_panel.set_endpoints(&mut app.s, ends);
            let (preview, draft) = (app.s.doc.clone(), app.array_panel.settings.clone());
            nodes(&ctx, &mut app);
            if choice == "Keep Editing" {
                app.shot = None;
                let mut input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 940.0))),
                    ..Default::default()
                };
                input.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
                let mut output = ctx.run_ui(input, |ui| app.frame(ui));
                output.textures_delta.clear();
                assert!(output.viewport_output.values().any(|viewport| viewport.commands.contains(&egui::ViewportCommand::CancelClose)));
            } else if apply {
                history_key(&ctx, &mut app, Key::Q, Modifiers::NONE);
            } else if dirty && crate::platform::OS != crate::platform::Os::Mac && !crate::platform::WEB {
                frame(&ctx, &mut app, vec![Event::Key {
                    key: Key::F, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::ALT,
                }]);
                let menu = nodes(&ctx, &mut app);
                let exit = menu.iter().find(|(_, n)| n.label().is_some_and(|label| label.starts_with("Exit")))
                    .unwrap_or_else(|| panic!("Exit menu item missing: {:?}", menu.iter().filter_map(|(_, n)| n.label()).collect::<Vec<_>>())).1.clone();
                click(&ctx, &mut app, &exit);
            } else {
                app.system_request(crate::platform::SystemRequest::Quit);
            }
            assert!(app.array_quit_pending && app.s.array_preview_active() && !app.tabs.asking());
            let button = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label() == Some(choice)).unwrap().1;
            click(&ctx, &mut app, &button);
            assert!(!app.array_quit_pending);
            if choice == "Keep Editing" {
                assert!(app.s.array_preview_active() && app.array_panel.can_step(false));
                assert!(app.s.doc == preview && app.array_panel.settings == draft);
                assert!(!app.allow_close && app.tabs.closing.is_none());
                continue;
            }
            assert!(!app.s.array_preview_active());
            assert_eq!(app.s.history_len(), steps + usize::from(apply));
            assert!(app.s.doc == if apply { preview } else { original });
            if dirty || apply {
                assert!(app.tabs.asking() && !app.allow_close);
                if !cancel_save {
                    let discard = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label() == Some("Don't Save")).unwrap().1;
                    click(&ctx, &mut app, &discard);
                    assert!(app.allow_close && app.tabs.closing.is_none());
                    continue;
                }
                let cancel = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label() == Some("Cancel")).unwrap().1;
                click(&ctx, &mut app, &cancel);
                assert!(!app.allow_close && app.tabs.closing.is_none() && !app.modal_open());
                assert!(!app.s.array_preview_active());
                if apply {
                    assert!(app.array_panel.settings.is_none());
                } else {
                    assert_eq!(app.array_panel.settings, draft);
                }
            } else {
                assert!(app.allow_close);
            }
        }
    }

    #[test]
    fn array_history_exits_keep_the_draft_and_leave_document_redo_intact() {
        let (mut app, ctx) = linear_app();
        app.s.execute("element.translate", &json!({"dx": 10, "dy": 0})).unwrap();
        app.s.undo().unwrap();
        let (base, source, undo_len, redo_len) = (app.s.doc.clone(), app.s.selection.clone(), app.s.history_len(), app.s.history().redo_len());
        for exit in ["Cancel", "Escape", "None"] {
            app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
            assert!(!app.array_panel.can_step(false) && !app.array_panel.can_step(true));
            nodes(&ctx, &mut app);
            let mut ends = app.array_panel.gizmo_endpoints().unwrap();
            ends.end[0] -= 40.0;
            app.array_panel.set_endpoints(&mut app.s, ends);
            let draft = app.array_panel.settings.clone();
            ends.end[1] += 10.0;
            app.array_panel.set_endpoints(&mut app.s, ends);
            history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
            assert_eq!(app.array_panel.settings, draft);
            assert!(app.array_panel.can_step(true));
            match exit {
                "Cancel" => {
                    let cancel = commit_control(&ctx, &mut app, "Cancel");
                    click(&ctx, &mut app, &cancel);
                }
                "Escape" => {
                    frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
                    frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
                }
                _ => app.array_panel.select_mode(&mut app.s, None),
            }
            assert!(!app.s.array_preview_active());
            assert!(app.s.doc == base && app.s.selection == source);
            assert_eq!(app.array_panel.settings, draft);
            assert_eq!(app.s.history_len(), undo_len);
            assert_eq!(app.s.history().redo_len(), redo_len);
        }
    }

    #[test]
    fn transform_stop_fields_drag_live_and_type_without_overwriting_other_stops() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let field = |app: &mut App, label: &str| {
            nodes(&ctx, app).into_iter().find(|(_, n)| n.label() == Some(label)).unwrap()
        };
        let value = |app: &App, i: usize| app.array_panel.settings.as_ref().unwrap().stops.get(i).unwrap().value.clone();
        let last = |app: &App| app.s.selected_elements().last().unwrap().1.transform;
        assert_eq!(value(&app, 0), ArrayPropertyStop::new(0.0, 1.0));
        let before = last(&app);
        let lock = field(&mut app, "Lock scale").1;
        click(&ctx, &mut app, &lock);
        let scale = field(&mut app, "Scale X").1.bounds().unwrap();
        let from = pos2(((scale.x0 + scale.x1) * 0.5) as f32, ((scale.y0 + scale.y1) * 0.5) as f32);
        drag_gizmo(&ctx, &mut app, from, vec2(40.0, 0.0), false);
        let first = value(&app, 0);
        assert!(first.scale[0] > 1.0);
        assert_eq!(first.scale[0], first.scale[1]);
        assert_eq!(last(&app), before);
        assert!(app.s.selected_elements().first().unwrap().1.transform.m[0] > 1.0);
        frame(&ctx, &mut app, vec![pointer_button(from + vec2(40.0, 0.0), false)]);

        let other = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label().is_some_and(|s| s.starts_with("Array stop 2 of"))).unwrap().1;
        click(&ctx, &mut app, &other);
        assert_eq!(field(&mut app, "Scale X").1.numeric_value(), Some(100.0));
        let rotation = field(&mut app, "Rotation").1;
        click(&ctx, &mut app, &rotation);
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false,
            modifiers: Modifiers::COMMAND }, Event::Text("720".to_owned())]);
        frame(&ctx, &mut app, vec![key(Key::Enter, true)]);
        assert_eq!(value(&app, 1).rotation, 720.0);
        assert_eq!(value(&app, 0), first);
        assert_eq!(value(&app, 1).alpha, 1.0);

        let transform = field(&mut app, "Transform").1;
        click(&ctx, &mut app, &transform);
        let collapsed = nodes(&ctx, &mut app);
        assert!(!collapsed.iter().any(|(_, n)| n.label() == Some("Rotation")));
        assert!(collapsed.iter().any(|(_, n)| n.role() == Role::SpinButton && n.label() == Some("Alpha")));

        let mut endpoints = app.array_panel.gizmo_endpoints().unwrap();
        endpoints.end[0] += 150.0;
        app.array_panel.set_endpoints(&mut app.s, endpoints);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        let mut radial = app.array_panel.settings.as_ref().unwrap().radial;
        radial.closed_loop = false;
        radial.spiral_offset = 44.0;
        radial.centre_drift = [20.0, -30.0];
        radial.sweep = -810.0;
        radial.rotate = true;
        app.array_panel.set_gizmo_geometry(&mut app.s, Geometry::Radial(radial), None);
        assert_eq!(value(&app, 0), first);
        assert_eq!(value(&app, 1), ArrayPropertyStop { rotation: 720.0, ..ArrayPropertyStop::new(1.0, 1.0) });
    }

    fn near(actual: [f32; 2], expected: [f32; 2]) {
        assert!((actual[0] - expected[0]).abs() < 0.01 && (actual[1] - expected[1]).abs() < 0.01, "{actual:?} != {expected:?}");
    }

    fn preview_ends(app: &App, ends: Endpoints) {
        let selected = app.s.selected_elements();
        let point = |e: &(_, std::sync::Arc<anicraft_engine::doc::Element>)| [e.1.transform.m[4], e.1.transform.m[5]];
        near(point(selected.first().unwrap()), ends.start);
        near(point(selected.last().unwrap()), ends.end);
    }

    #[test]
    fn changing_selection_discards_a_cancelled_draft_even_when_the_panel_is_closed() {
        let (mut app, ctx) = linear_app();
        let (_, first) = app.s.selection.first().unwrap();
        let origin = source_origin(&app.s).unwrap();
        instance(&mut app.s, 475.0, "Second");
        let (_, second) = app.s.selection.first().unwrap();
        app.s.select_only([first]);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        let settings = app.array_panel.settings.as_mut().unwrap();
        settings.count = 17;
        settings.radial.closed_loop = false;
        settings.radial.sweep = -720.0;
        settings.radial.spiral_offset = 35.0;
        app.array_panel.refresh(&mut app.s);
        app.array_panel.dim_stage = false;
        app.array_panel.show_gizmo = false;
        let action = commit_control(&ctx, &mut app, "Cancel");
        click(&ctx, &mut app, &action);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().radial.spiral_offset, 35.0);

        crate::dock_ui::apply(&mut app, vec![crate::dock_ui::Act::Close(Panel::Array)]);
        app.s.select_only([second]);
        frame(&ctx, &mut app, vec![]);
        assert!(app.array_panel.settings.is_none());
        app.s.select_only([first]);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert_eq!(app.array_panel.settings.as_ref(), Some(&Settings::at(origin)));
        assert!(!app.array_panel.dim_stage() && !app.array_panel.shows_registrations());
    }

    #[test]
    fn filter_cards_share_expansion_across_stops_and_modes_by_slot() {
        let (mut app, ctx) = linear_app();
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": [
            {"type": "blur", "blur_x": 7}, {"type": "blur", "blur_x": 20},
        ]})).unwrap();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let shown = |app: &mut App, name: &str| nodes(&ctx, app).into_iter()
            .find(|(_, n)| n.label() == Some(name)).map(|(_, n)| n);
        let press = |app: &mut App, name: &str| {
            let header = shown(app, name).unwrap();
            click(&ctx, app, &header);
        };
        let initial = app.array_panel.settings.clone();
        let preview = app.s.doc.clone();
        for slot in ["Blur 1 blur X", "Blur 2 blur X"] { assert!(shown(&mut app, slot).is_none()); }
        press(&mut app, "Blur 1");
        assert_eq!(shown(&mut app, "Blur 1 blur X").unwrap().numeric_value(), Some(7.0));
        assert!(shown(&mut app, "Blur 2 blur X").is_none());
        press(&mut app, "Blur 2");
        assert!(shown(&mut app, "Blur 1 blur X").is_some());
        assert_eq!(shown(&mut app, "Blur 2 blur X").unwrap().numeric_value(), Some(20.0));
        press(&mut app, "Blur 1");
        assert_eq!(app.s.doc, preview);
        assert!(!app.array_panel.can_step(false));
        let last = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label().is_some_and(|label| label.starts_with("Array stop 2 of"))).unwrap().1;
        click(&ctx, &mut app, &last);
        assert!(shown(&mut app, "Blur 1 blur X").is_none());
        assert!(shown(&mut app, "Blur 2 blur X").is_some());
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert!(shown(&mut app, "Blur 1 blur X").is_none());
        assert!(shown(&mut app, "Blur 2 blur X").is_some());
        assert_eq!(app.array_panel.settings, initial);
        app.array_panel.finish(&mut app.s, false);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        for slot in ["Blur 1 blur X", "Blur 2 blur X"] { assert!(shown(&mut app, slot).is_none()); }
        assert_eq!(app.array_panel.settings, initial);
    }

    #[test]
    fn radial_panel_keeps_winding_in_open_mode_and_limits_a_closed_loop() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        // Use the collapsible stop editor to give the open Radial fields room in this viewport.
        let colour = nodes(&ctx, &mut app).into_iter().find(|(_, n)| n.label() == Some("Colour Effect")).unwrap().1;
        click(&ctx, &mut app, &colour);
        let set_field = |app: &mut App, label: &str, value: f64| {
            let fields = nodes(&ctx, app);
            let input = fields.iter().find(|(_, n)| n.role() == Role::SpinButton && n.label() == Some(label)).unwrap().0;
            set_number(&ctx, app, input, value);
        };
        let choice = |app: &mut App, label| {
            let shown = nodes(&ctx, app);
            shown.into_iter().find(|(_, n)| n.label() == Some(label)).unwrap().1
        };
        let shows_offset = |app: &mut App| nodes(&ctx, app).iter().any(|(_, n)| n.value() == Some("Spiral Offset"));
        assert!(app.array_panel.settings.as_ref().unwrap().radial.closed_loop);
        assert_eq!(choice(&mut app, "Circle").toggled(), Some(egui::accesskit::Toggled::True));
        assert_eq!(choice(&mut app, "Spiral").toggled(), Some(egui::accesskit::Toggled::False));
        assert!(!shows_offset(&mut app));
        let spiral = choice(&mut app, "Spiral");
        click(&ctx, &mut app, &spiral);
        assert_eq!(choice(&mut app, "Circle").toggled(), Some(egui::accesskit::Toggled::False));
        assert_eq!(choice(&mut app, "Spiral").toggled(), Some(egui::accesskit::Toggled::True));
        let radial = app.array_panel.settings.as_ref().unwrap().radial;
        let last = app.s.selected_elements().last().unwrap().1.transform.translation();
        assert!((last.x - radial.centre[0] - radial.radius - 48.0).abs() < 0.001);
        history_key(&ctx, &mut app, Key::Z, Modifiers::NONE);
        assert_eq!(choice(&mut app, "Circle").toggled(), Some(egui::accesskit::Toggled::True));
        history_key(&ctx, &mut app, Key::Y, Modifiers::NONE);
        assert_eq!(choice(&mut app, "Spiral").toggled(), Some(egui::accesskit::Toggled::True));
        assert!(shows_offset(&mut app));
        set_field(&mut app, "Spiral Offset", 15.0);
        set_field(&mut app, "Centre Drift X", 32.0);
        set_field(&mut app, "Centre Drift Y", -18.0);
        for sweep in [358.0, 360.0, 362.0, -360.0, 720.0, -1080.0] {
            set_field(&mut app, "Sweep", sweep);
            let radial = &app.array_panel.settings.as_ref().unwrap().radial;
            assert_eq!(radial.sweep, sweep as f32);
            assert!(!radial.closed_loop);
            let last = app.s.selected_elements().last().unwrap().1.transform.translation();
            let [x, y] = radial.centre_after_turns(f64::from(radial.sweep.abs()) / 360.0);
            let radius = (last.x - x as f32).hypot(last.y - y as f32);
            assert!((radius - radial.radius - 15.0 * radial.sweep.abs() / 360.0).abs() < 0.001);
        }
        let circle = choice(&mut app, "Circle");
        click(&ctx, &mut app, &circle);
        assert_eq!(choice(&mut app, "Circle").toggled(), Some(egui::accesskit::Toggled::True));
        assert_eq!(choice(&mut app, "Spiral").toggled(), Some(egui::accesskit::Toggled::False));
        assert!(!shows_offset(&mut app));
        assert!(!nodes(&ctx, &mut app).iter().any(|(_, n)| n.label() == Some("Centre Drift X")));
        let radial = &app.array_panel.settings.as_ref().unwrap().radial;
        assert!(radial.closed_loop);
        assert_eq!(radial.sweep, -360.0);
        let spiral = choice(&mut app, "Spiral");
        click(&ctx, &mut app, &spiral);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().radial.spiral_offset, 15.0);
        assert_eq!(app.array_panel.settings.as_ref().unwrap().radial.centre_drift, [32.0, -18.0]);
    }

    #[test]
    fn radial_stage_handles_update_fields_and_keep_cancel_commit_and_undo() {
        use anicraft_engine::geom::Vec2 as Point;
        let (mut app, ctx) = linear_app();
        let doc = app.s.doc.clone();
        let selection = app.s.selection.clone();
        let steps = app.s.history_len();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        let radial = Radial { centre: [260.0, 220.0], radius: 80.0, start: 0.0, sweep: 90.0,
            closed_loop: false, spiral_offset: 0.0, centre_drift: [70.0, -90.0], rotate: false };
        app.array_panel.set_gizmo_geometry(&mut app.s, Geometry::Radial(radial), None);
        nodes(&ctx, &mut app);

        let current = |app: &App| app.array_panel.settings.as_ref().unwrap().radial;
        let drag_point = |app: &mut App, from: Point, delta: Vec2| {
            let pos = screen(app, [from.x, from.y]);
            let zoom = app.view.zoom;
            drag_gizmo(&ctx, app, pos, delta * zoom, true);
        };
        drag_point(&mut app, Point::new(260.0, 220.0), vec2(28.0, -15.0));
        let shifted = current(&app);
        near(shifted.centre, [288.0, 205.0]);
        drag_point(&mut app, shifted.point(0.5), vec2(20.0, 35.0));
        let shifted = current(&app);
        near(shifted.centre, [308.0, 240.0]);
        drag_point(&mut app, shifted.point(0.0), vec2(0.0, -60.0));
        let started = current(&app);
        assert!((started.radius - 100.0).abs() < 0.01 && (started.start + 36.8699).abs() < 0.01);
        assert_eq!(started.sweep, radial.sweep);

        let [x, y] = started.centre_after_turns(1.0);
        drag_point(&mut app, Point::new(x as f32, y as f32), vec2(25.0, 20.0));
        let drifting = current(&app);
        near(drifting.centre_drift, [95.0, -70.0]);
        assert_eq!(drifting.spiral_offset, started.spiral_offset);
        assert_eq!(drifting.sweep, started.sweep);

        let [x, y] = drifting.centre_after_turns(0.5);
        drag_point(&mut app, Point::new(x as f32, y as f32), vec2(-12.0, 16.0));
        let translated = current(&app);
        near(translated.centre, [296.0, 256.0]);
        assert_eq!(translated.centre_drift, drifting.centre_drift);

        let from = translated.point(1.0);
        let target = Radial { sweep: 120.0, spiral_offset: 60.0, ..translated }.point(1.0);
        drag_point(&mut app, from, vec2(target.x - from.x, target.y - from.y));
        let ended = current(&app);
        assert!((ended.sweep - 120.0).abs() < 0.01 && (ended.spiral_offset - 60.0).abs() < 0.05);
        assert_eq!(ended.centre_drift, translated.centre_drift);
        let last = app.s.selected_elements().last().unwrap().1.transform.translation();
        near([last.x, last.y], [target.x, target.y]);

        let fields = nodes(&ctx, &mut app);
        let (_, offset) = fields.iter().find(|(_, n)| n.label() == Some("Spiral Offset")).unwrap();
        assert!((offset.numeric_value().unwrap() - 60.0).abs() < 0.05);
        let (_, drift_x) = fields.iter().find(|(_, n)| n.label() == Some("Centre Drift X")).unwrap();
        assert!((drift_x.numeric_value().unwrap() - 95.0).abs() < 0.05);
        let cancel = commit_control(&ctx, &mut app, "Cancel");
        click(&ctx, &mut app, &cancel);
        assert!(app.s.doc == doc && app.s.selection == selection);
        assert_eq!(app.s.history_len(), steps);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        assert_eq!(current(&app), ended);
        let action = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &action);
        assert!(app.array_panel.gizmo_geometry().is_none());
        assert_eq!(app.s.history_len(), steps + 1);
        app.s.undo().unwrap();
        assert!(app.s.doc == doc && app.s.selection == selection);
    }

    #[test]
    fn stage_drags_change_endpoints_and_numeric_fields_through_the_same_preview() {
        let (mut app, ctx) = linear_app();
        let (doc, selection, steps) = (app.s.doc.clone(), app.s.selection.clone(), app.s.history_len());
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        nodes(&ctx, &mut app);
        let initial = app.array_panel.gizmo_endpoints().unwrap();
        let zoom = app.view.zoom;
        let start = screen(&app, initial.start);
        drag_gizmo(&ctx, &mut app, start, vec2(18.0, 24.0) * zoom, false);
        let moved_start = app.array_panel.gizmo_endpoints().unwrap();
        near(moved_start.start, [initial.start[0] + 18.0, initial.start[1] + 24.0]);
        assert_eq!(moved_start.end, initial.end);
        preview_ends(&app, moved_start);
        let released = screen(&app, moved_start.start);
        frame(&ctx, &mut app, vec![pointer_button(released, false)]);

        let end = screen(&app, moved_start.end);
        drag_gizmo(&ctx, &mut app, end, vec2(32.0, 18.0) * zoom, true);
        let moved_end = app.array_panel.gizmo_endpoints().unwrap();
        near(moved_end.end, [initial.end[0] + 32.0, initial.end[1] + 18.0]);
        assert_eq!(moved_end.start, moved_start.start);

        let mid = screen(&app, moved_end.start).lerp(screen(&app, moved_end.end), 0.5) + vec2(0.0, 8.0);
        drag_gizmo(&ctx, &mut app, mid, vec2(20.0, 30.0) * zoom, true);
        let moved_bar = app.array_panel.gizmo_endpoints().unwrap();
        near(moved_bar.start, [moved_end.start[0] + 20.0, moved_end.start[1] + 30.0]);
        near(moved_bar.end, [moved_end.end[0] + 20.0, moved_end.end[1] + 30.0]);
        preview_ends(&app, moved_bar);

        let fields = nodes(&ctx, &mut app);
        let (end_x, node) = fields.iter().find(|(_, n)| n.label() == Some("End X")).unwrap();
        assert!((node.numeric_value().unwrap() - f64::from(moved_bar.end[0])).abs() < 0.01);
        set_number(&ctx, &mut app, *end_x, 620.0);
        let typed = app.array_panel.gizmo_endpoints().unwrap();
        assert_eq!(typed.end[0], 620.0);
        preview_ends(&app, typed);

        // Leave room for the full distribution section in this dock viewport.
        let stops = fields.iter().find(|(_, node)| node.label() == Some("Property stops")).unwrap().1.clone();
        click(&ctx, &mut app, &stops);
        let fields = nodes(&ctx, &mut app);
        let count = fields.iter().find(|(_, n)| n.label() == Some("Instances") && n.numeric_value() == Some(10.0))
            .unwrap_or_else(|| panic!("Instances control missing from Array accessibility tree: {:?}", fields.iter()
                .filter_map(|(_, n)| n.label().map(|label| (n.role(), label, n.numeric_value()))).collect::<Vec<_>>())).0;
        set_number(&ctx, &mut app, count, 1.0);
        assert_eq!(app.s.selection.len(), 1);
        assert_eq!(app.array_panel.gizmo_endpoints(), Some(typed));
        assert_eq!(typed.spacing(1), None);
        set_number(&ctx, &mut app, count, 5.0);
        assert_eq!(app.s.selection.len(), 5);
        assert_eq!(app.array_panel.gizmo_endpoints(), Some(typed));
        preview_ends(&app, typed);

        let action = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &action);
        assert_eq!(app.s.history_len(), steps + 1);
        assert!(app.array_panel.gizmo_endpoints().is_none());
        app.s.undo().unwrap();
        assert!(app.s.doc == doc && app.s.selection == selection);
    }

    #[test]
    fn commit_resets_configuration_at_the_next_symbols_current_position() {
        let (mut app, ctx) = linear_app();
        let (_, first) = app.s.selection.first().unwrap();
        instance(&mut app.s, 530.0, "Second");
        let (_, second) = app.s.selection.first().unwrap();
        let original_origin = source_origin(&app.s).unwrap();
        app.s.select_only([first]);
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let config = app.array_panel.settings.as_mut().unwrap();
        config.count = 4;
        config.linear = Endpoints { start: [73.0, 222.0], end: [381.0, 129.0] };
        config.radial.radius = 67.0;
        config.backwards = true;
        config.stops.insert(1, PropertyStop::new(2, 0.25, 0.4));
        app.array_panel.refresh(&mut app.s);

        for label in ["Hide Array Handles", "Dim Stage"] {
            let shown = nodes(&ctx, &mut app);
            click(&ctx, &mut app, &shown.iter().find(|(_, n)| n.label() == Some(label)).unwrap().1);
        }
        let commit = commit_control(&ctx, &mut app, "Commit");
        click(&ctx, &mut app, &commit);
        assert_eq!(app.s.selection.len(), 4);
        assert_eq!(app.array_panel.mode(), None);
        assert!(app.array_panel.settings.is_none() && !app.s.array_preview_active());

        app.s.select_only([second]);
        app.s.execute("element.translate", &json!({"dx": 42, "dy": 32})).unwrap();
        let moved_origin = [original_origin[0] + 42.0, original_origin[1] + 32.0];
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(app.array_panel.settings.as_ref(), Some(&Settings::at(moved_origin)));
        assert_eq!(app.s.selection.len(), 10);
        assert_eq!(app.s.selection.first().unwrap().1, second);
        preview_ends(&app, Settings::at(moved_origin).linear);
        assert!(!app.array_panel.dim_stage() && !app.array_panel.shows_registrations());
    }

    #[test]
    fn direction_length_and_each_spacing_field_edit_the_canonical_end() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let start = app.array_panel.gizmo_endpoints().unwrap().start;
        let input = |ctx: &egui::Context, app: &mut App, label: &str, value: f64| {
            let target = nodes(ctx, app).into_iter().find(|(_, n)| n.label() == Some(label)).unwrap().0;
            set_number(ctx, app, target, value);
            let ends = app.array_panel.gizmo_endpoints().unwrap();
            assert_eq!(ends.start, start);
            ends
        };
        near(input(&ctx, &mut app, "Direction", 90.0).extent(), [0.0, 324.0]);
        near(input(&ctx, &mut app, "Length", 90.0).extent(), [0.0, 90.0]);
        near(input(&ctx, &mut app, "Spacing X", 7.0).extent(), [63.0, 90.0]);
        let ends = input(&ctx, &mut app, "Spacing Y", -5.0);
        near(ends.extent(), [63.0, -45.0]);
        preview_ends(&app, ends);

        app.array_panel.settings.as_mut().unwrap().count = 1;
        app.array_panel.refresh(&mut app.s);
        let fields = nodes(&ctx, &mut app);
        for label in ["Spacing X", "Spacing Y"] {
            assert!(fields.iter().find(|(_, n)| n.label() == Some(label)).unwrap().1.is_disabled());
        }
        near(input(&ctx, &mut app, "Length", 0.0).extent(), [0.0, 0.0]);
        let fields = nodes(&ctx, &mut app);
        assert!(fields.iter().find(|(_, n)| n.label() == Some("Direction")).unwrap().1.is_disabled());
        near(input(&ctx, &mut app, "Length", 36.0).extent(), [36.0, 0.0]);
        assert_eq!(app.s.selection.len(), 1);
    }

    #[test]
    fn properties_adjust_color_sliders_click_to_set_and_undo() {
        let (mut app, ctx) = linear_app();
        app.dock.right.size = Some(450.0);
        app.show_panel(Panel::Properties);
        for group in &mut app.dock.right.groups {
            group.folded = !group.tabs.contains(&Panel::Properties);
        }
        app.s.execute("symbol.set_kind", &json!({"kind": "movie_clip"})).unwrap();
        app.s.execute("element.set_filters", &json!({"filters": [{"type": "adjust_color"}]})).unwrap();
        let before = app.s.doc.clone();
        let steps = app.s.history_len();
        let controls = nodes(&ctx, &mut app);
        for (label, limit) in [("Brightness", 100.0), ("Contrast", 100.0), ("Saturation", 100.0), ("Hue", 180.0)] {
            let (_, slider) = controls.iter().find(|(_, n)| n.label() == Some(label) && n.role() == Role::Slider)
                .unwrap_or_else(|| panic!("missing {label} slider"));
            assert_eq!(slider.min_numeric_value(), Some(-limit));
            assert_eq!(slider.max_numeric_value(), Some(limit));
            assert!(controls.iter().any(|(_, n)| n.label() == Some(label) && n.role() == Role::SpinButton));
        }
        let hue = controls.iter().find(|(_, n)| n.label() == Some("Hue") && n.role() == Role::Slider).unwrap().1.bounds().unwrap();
        let pos = pos2((hue.x0 + (hue.x1 - hue.x0) * 0.8) as f32, ((hue.y0 + hue.y1) / 2.0) as f32);
        frame(&ctx, &mut app, vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            frame(&ctx, &mut app, vec![pointer_button(pos, pressed)]);
        }
        let selected = app.s.selected_elements();
        assert!(matches!(selected[0].1.kind.filters().unwrap()[0].filter, Filter::AdjustColor { hue, .. } if hue > 50.0));
        assert_eq!(app.s.history_len(), steps + 1);
        let after = app.s.doc.clone();
        app.s.undo().unwrap();
        assert!(app.s.doc == before);
        app.s.redo().unwrap();
        assert!(app.s.doc == after);
    }

    #[test]
    fn array_with_hidden_handles_does_not_buffer_pen_points() {
        let (mut app, ctx) = linear_app();
        app.tool = crate::app::Tool::Pen;
        assert!(app.pen.is_none());
        let (doc, selection, steps) = (app.s.doc.clone(), app.s.selection.clone(), app.s.history_len());
        for label in ["Array type", "Linear", "Hide Array Handles"] {
            let controls = nodes(&ctx, &mut app);
            click(&ctx, &mut app, &controls.iter().find(|(_, n)| n.label() == Some(label)).unwrap().1);
        }
        assert!(app.s.array_preview_active() && app.array_panel.gizmo_geometry().is_none());
        let positions = [-100.0, 100.0].map(|dx| app.stage_cache.canvas.center() + vec2(dx, 60.0));
        let click_stage = |app: &mut App| {
            for pos in positions {
                frame(&ctx, app, vec![Event::PointerMoved(pos)]);
                for pressed in [true, false] {
                    frame(&ctx, app, vec![pointer_button(pos, pressed)]);
                }
            }
        };
        click_stage(&mut app);
        let buffered_during_array = app.pen.is_some();
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        assert!(!app.s.array_preview_active() && app.tool == crate::app::Tool::Pen);
        let buffered_after_cancel = app.pen.is_some();
        app.tool = crate::app::Tool::Select;
        frame(&ctx, &mut app, vec![]);
        assert!(!buffered_during_array && !buffered_after_cancel, "Array clicks buffered an ordinary Pen path");
        assert!(app.pen.is_none() && app.s.doc == doc && app.s.selection == selection);
        assert_eq!(app.s.history_len(), steps);

        app.tool = crate::app::Tool::Pen;
        click_stage(&mut app);
        assert!(app.pen.is_some(), "the same clicks reach the Pen in ordinary editing");
    }

    #[test]
    fn radial_visibility_toggle_controls_instance_registrations() {
        let (mut app, ctx) = linear_app();
        app.array_panel.select_mode(&mut app.s, Some(Mode::Radial));
        let fields = nodes(&ctx, &mut app);
        assert!(app.array_panel.shows_registrations() && matches!(app.array_panel.gizmo_geometry(), Some(Geometry::Radial(_))));
        assert_eq!(app.s.selected_elements().len(), 10);
        click(&ctx, &mut app, &fields.iter().find(|(_, n)| n.label() == Some("Hide Array Handles")).unwrap().1);
        assert!(!app.array_panel.shows_registrations());
        assert!(app.array_panel.gizmo_geometry().is_none());
        assert!(app.s.array_preview_active() && app.s.selected_elements().len() == 10);
    }

    #[test]
    fn hiding_the_gizmo_keeps_preview_and_numeric_editing_and_exit_restores_the_source() {
        let (mut app, ctx) = linear_app();
        let (doc, selection, steps) = (app.s.doc.clone(), app.s.selection.clone(), app.s.history_len());
        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        let shown = nodes(&ctx, &mut app);
        let ends = app.array_panel.gizmo_endpoints().unwrap();
        let preview = app.s.doc.clone();
        click(&ctx, &mut app, &shown.iter().find(|(_, n)| n.label() == Some("Hide Array Handles")).unwrap().1);
        assert!(app.array_panel.gizmo_endpoints().is_none() && app.s.array_preview_active() && app.s.doc == preview);
        let start = screen(&app, ends.start);
        drag_gizmo(&ctx, &mut app, start, vec2(40.0, 30.0), true);
        assert!(app.s.doc == preview);
        let shown = nodes(&ctx, &mut app);
        let (end_y, _) = shown.iter().find(|(_, n)| n.label() == Some("End Y")).unwrap();
        set_number(&ctx, &mut app, *end_y, 129.0);
        assert!(app.s.doc != preview && app.s.array_preview_active());

        let shown = nodes(&ctx, &mut app);
        click(&ctx, &mut app, &shown.iter().find(|(_, n)| n.label() == Some("Show Array Handles")).unwrap().1);
        let ends = app.array_panel.gizmo_endpoints().unwrap();
        assert_eq!(ends.end[1], 129.0);
        preview_ends(&app, ends);
        let start = screen(&app, ends.start);
        drag_gizmo(&ctx, &mut app, start, vec2(40.0, 30.0), false);
        assert!(app.array_panel.gizmo_drag.is_some());
        let recovery = app.array_panel.settings.clone();
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        assert!(app.array_panel.gizmo_drag.is_none() && !app.s.array_preview_active());
        assert_eq!(app.array_panel.settings, recovery);
        frame(&ctx, &mut app, vec![pointer_button(start + vec2(40.0, 30.0), false), key(Key::Escape, false)]);
        assert!(app.s.doc == doc && app.s.selection == selection);

        app.array_panel.select_mode(&mut app.s, Some(Mode::Linear));
        assert_eq!(app.array_panel.settings, recovery);
        app.array_panel.set_endpoints(&mut app.s, ends);
        let recovery = app.array_panel.settings.clone();
        let action = commit_control(&ctx, &mut app, "Cancel");
        click(&ctx, &mut app, &action);
        assert_eq!(app.array_panel.settings, recovery);
        assert!(app.s.doc == doc && app.s.selection == selection);
        assert_eq!(app.s.history_len(), steps);
    }

    #[test]
    fn real_controls_enter_preview_and_escape_gives_child_controls_priority() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut s = Session::default();
        instance(&mut s, 50.0, "Source");
        let (doc, selection, steps) = (s.doc.clone(), s.selection.clone(), s.history_len());
        let mut app = App::with_context(&ctx, s, Some(std::path::PathBuf::from("unused.png")));
        app.show_panel(Panel::Array);
        let combo = |nodes: &[(NodeId, Node)], value| {
            nodes.iter().find(|(_, n)| n.role() == Role::ComboBox && n.value() == Some(value) && n.label() == Some("Array type")).unwrap().1.clone()
        };
        let named = |nodes: &[(NodeId, Node)], label| nodes.iter().find(|(_, n)| n.label() == Some(label)).unwrap().1.clone();
        let idle = nodes(&ctx, &mut app);
        assert!(app.s.doc == doc && !app.s.array_preview_active());
        click(&ctx, &mut app, &combo(&idle, "None"));
        assert!(egui::Popup::is_any_open(&ctx), "the Array type dropdown opened");
        let choices = nodes(&ctx, &mut app);
        assert!(named(&choices, "Along Path").is_disabled());
        let grid = named(&choices, "Grid");
        assert!(!grid.is_disabled());
        click(&ctx, &mut app, &grid);
        assert_eq!(app.s.selection.len(), 9);
        let grid_controls = nodes(&ctx, &mut app);
        click(&ctx, &mut app, &combo(&grid_controls, "Grid"));
        let choices = nodes(&ctx, &mut app);
        click(&ctx, &mut app, &named(&choices, "Linear"));
        assert_eq!(app.s.selection.len(), 10, "changing the dropdown previews immediately");
        let shown = nodes(&ctx, &mut app);
        assert!(shown.iter().any(|(_, n)| n.label() == Some("Colour Effect")));
        click(&ctx, &mut app, &combo(&shown, "Linear"));
        assert!(egui::Popup::is_any_open(&ctx));
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        assert!(app.s.array_preview_active() && !egui::Popup::is_any_open(&ctx));

        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        let shown = nodes(&ctx, &mut app);
        click(&ctx, &mut app, &named(&shown, "Array stop position"));
        nodes(&ctx, &mut app);
        assert!(ctx.text_edit_focused());
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        assert!(app.s.array_preview_active() && !ctx.text_edit_focused());

        app.show_panel(Panel::Properties);
        nodes(&ctx, &mut app);
        assert!(app.s.array_preview_active() && !app.panel_visible(Panel::Array));
        crate::dock_ui::apply(&mut app, vec![crate::dock_ui::Act::Close(Panel::Array)]);
        let hidden = nodes(&ctx, &mut app);
        assert!(app.s.array_preview_active() && !app.panel_visible(Panel::Array));
        assert!(hidden.iter().any(|(_, n)| n.label() == Some("Show Array")));
        assert!(app.run("edit.delete", json!({})).is_none());
        assert!(app.perform("file.new", json!({})).is_none());
        frame(&ctx, &mut app, vec![key(Key::Escape, false)]);
        frame(&ctx, &mut app, vec![key(Key::Escape, true)]);
        assert!(app.s.doc == doc && app.s.selection == selection && !app.s.in_gesture());
        assert_eq!(app.s.history_len(), steps);
        assert_eq!(app.array_panel.mode(), None);
    }
}
