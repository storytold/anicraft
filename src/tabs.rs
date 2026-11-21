//! Several documents open at once, each in a tab above the stage (ADR 0112).
//!
//! The document being edited is where it always was: its [`Session`] is `App::s` and its view
//! state is in `App`'s own fields, so every panel and tool keeps reading the same places. The
//! other open documents are *parked*: each tab that isn't shown holds its whole session (the
//! document, its undo history, selection and playhead), its view state ([`DocView`]: zoom and
//! pan, timeline scroll, Library and Motion Editor state, …) and its autosave. Showing another
//! tab exchanges the two, and hands the editor's share (the clipboards, the tool style, plug-ins,
//! recent files) from one session to the other, so a copy made in one document pastes into the
//! next (`Session::take_shared`).
//!
//! Which documents are open is the window's state, like zoom and the tool in hand: it isn't in
//! any document and isn't a command. The CLI and MCP stay one document per session.

use crate::a11y;
use crate::app::{App, Drag, LibraryItem, StageView};
use crate::autosave::DocSaver;
use crate::platform;
use crate::theme::{self, T, Type, font};
use anicraft_engine::Session;
use anicraft_engine::doc::{Document, ElementId, LayerId};
use egui::{Id, Rect, Sense, Stroke, StrokeKind, WidgetType, pos2, vec2};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The most documents open at once: each keeps its undo history in memory and its own autosave
/// writer.
pub const MAX_TABS: usize = 32;
/// The bar's height (points).
pub const BAR_HEIGHT: f32 = theme::TARGET + 2.0;
/// A tab is never narrower or wider than this, whatever its name.
const TAB_MIN: f32 = 96.0;
const TAB_MAX: f32 = 240.0;
/// Space the close button takes at a tab's right end, and the unsaved dot at its left.
const CLOSE_W: f32 = 22.0;
const DOT_W: f32 = 12.0;
const PAD: f32 = 10.0;

/// Declares [`DocView`]: the fields of [`App`] that belong to the document being edited rather
/// than to the window. Each is named as in `App`, with the value a newly opened document gets.
macro_rules! doc_view {
    ($app:ident; $($field:ident: $ty:ty = $fresh:expr),* $(,)?) => {
        /// One document's view state while another is shown (see the module docs).
        pub struct DocView { $($field: $ty),* }

        impl DocView {
            /// The view of a document that has just been opened.
            fn fresh($app: &App) -> DocView {
                DocView { $($field: $fresh),* }
            }

            /// Exchanges this view with the one in `app`.
            fn swap(&mut self, app: &mut App) {
                $(std::mem::swap(&mut self.$field, &mut app.$field);)*
            }
        }
    };
}

doc_view! { app;
    view: StageView = StageView { zoom: 1.0, pan: egui::Vec2::ZERO, fit: true, fitted: None, clip_to_stage: app.view.clip_to_stage },
    camera_preview: bool = false,
    tl_scroll: f32 = 0.0,
    tl_cell_w: f32 = crate::timeline::CELL_DEFAULT,
    tl_button_zoom: Option<(f32, f32)> = None,
    tl_vscroll: f32 = 0.0,
    tl_last_layer: Option<LayerId> = None,
    tl_collapsed: HashSet<LayerId> = HashSet::new(),
    timeline_focus: bool = false,
    frame_sel: Option<crate::timeline::FrameSel> = None,
    library_sel: Option<LibraryItem> = None,
    library: crate::library::LibraryView = Default::default(),
    explorer: crate::movie_explorer::ExplorerView = Default::default(),
    actions: crate::actions_panel::State = Default::default(),
    points: Vec<crate::pathtools::PointRef> = Vec::new(),
    raw_anchors: Vec<anicraft_engine::geom::Vec2> = Vec::new(),
    raw_ends: Vec<anicraft_engine::geom::Vec2> = Vec::new(),
    width_sel: Option<(ElementId, usize)> = None,
    bone_sel: Option<anicraft_engine::doc::BoneId> = None,
    warp_pin: Option<(ElementId, anicraft_engine::doc::warp::PinId)> = None,
    motion_anchors: Vec<usize> = Vec::new(),
    xf: crate::xform::State = Default::default(),
    envelope_open: bool = false,
    me_prop: anicraft_engine::doc::Property = anicraft_engine::doc::Property::X,
    me_key: Option<u32> = None,
    me_sel: Vec<(anicraft_engine::doc::Property, u32)> = Vec::new(),
    me_sel_at: (u64, u32) = (0, 0),
    me_vscroll: f32 = 0.0,
    me_styles: Vec<anicraft_engine::doc::ColorStyle> = Vec::new(),
    me_open: Vec<(u8, anicraft_engine::doc::FilterColor)> = Vec::new(),
}

/// An open document that isn't the one being edited.
pub struct Parked {
    pub s: Session,
    view: DocView,
    saver: DocSaver,
}

/// One tab.
struct Slot {
    /// The tab's identity while it is open (never reused in a run).
    id: u64,
    /// Which untitled document it is (1: "Untitled", 2: "Untitled 2", …), for its name until it
    /// is saved.
    untitled: u32,
    /// The document the app started with, untouched: opening a file takes its place instead of
    /// a new tab, as a word processor's blank page does.
    placeholder: bool,
    /// `None` for the document being edited (it is in `App`).
    parked: Option<Box<Parked>>,
}

/// What a tab shows.
#[derive(Clone, Debug, PartialEq)]
pub struct TabInfo {
    pub id: u64,
    pub name: String,
    pub dirty: bool,
    pub path: Option<PathBuf>,
    pub active: bool,
}

/// Closing documents, or the window: which tabs are still to go, and what is being waited for.
#[derive(Clone, Debug, PartialEq)]
pub struct CloseFlow {
    /// Tabs to close, in order (for a quit: the ones with unsaved changes).
    pub ids: Vec<u64>,
    /// The window is closing: one question for every document (ADR 0004).
    pub quit: bool,
    /// The answer was Save: documents are being saved, one after another when quitting.
    saving: bool,
    /// The tab whose save was started (its Save As may still be asking for a name).
    asked: Option<u64>,
}

/// What the close question was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

/// The open documents.
pub struct Tabs {
    slots: Vec<Slot>,
    active: usize,
    next_id: u64,
    /// The tab being dragged along the bar.
    drag: Option<u64>,
    pub closing: Option<CloseFlow>,
    /// How far the tabs are scrolled when they don't fit (points), and whether the shown one
    /// should be brought into view.
    scroll: f32,
    reveal: bool,
    /// Where each tab was drawn last frame (tests and screenshots).
    pub rects: Vec<(u64, Rect)>,
}

impl Tabs {
    /// One tab: the document the app starts with. `placeholder`: it is the sample or a blank
    /// page rather than a file the user opened.
    pub fn new(placeholder: bool) -> Tabs {
        Tabs { slots: vec![Slot { id: 1, untitled: 1, placeholder, parked: None }], active: 0, next_id: 2, drag: None, closing: None, scroll: 0.0, reveal: false, rects: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// The shown tab's place in the bar.
    #[cfg(test)]
    pub fn active(&self) -> usize {
        self.active
    }

    /// The shown tab's id (it stays the tab's as tabs open, close and move).
    pub fn active_id(&self) -> u64 {
        self.slots.get(self.active).map_or(0, |s| s.id)
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.slots.iter().position(|s| s.id == id)
    }

    /// The document the app started with was opened by the user after all (a file named on the
    /// command line): it keeps its tab.
    pub fn keep_first(&mut self) {
        for s in &mut self.slots {
            s.placeholder = false;
        }
    }


    /// The close question is showing (a modal: shortcuts pause).
    pub fn asking(&self) -> bool {
        self.closing.as_ref().is_some_and(|f| !f.saving)
    }
}

/// A document's name on its tab: the file's, or "Untitled" (numbered from the second on).
fn name_of(untitled: u32, path: Option<&Path>) -> String {
    match path.and_then(Path::file_name) {
        Some(n) => n.to_string_lossy().into_owned(),
        None if untitled <= 1 => tr!("Untitled").to_string(),
        None => tr!("Untitled {n}", n = untitled),
    }
}

/// The session of tab `i`: the app's for the shown one.
fn session(app: &App, i: usize) -> Option<&Session> {
    let slot = app.tabs.slots.get(i)?;
    Some(slot.parked.as_ref().map_or(&app.s, |p| &p.s))
}

/// What tab `i` shows.
pub fn info(app: &App, i: usize) -> Option<TabInfo> {
    let slot = app.tabs.slots.get(i)?;
    let s = session(app, i)?;
    Some(TabInfo { id: slot.id, name: name_of(slot.untitled, s.path.as_deref()), dirty: s.is_dirty(), path: s.path.clone(), active: i == app.tabs.active })
}

/// Every tab, in the bar's order.
pub fn infos(app: &App) -> Vec<TabInfo> {
    (0..app.tabs.len()).filter_map(|i| info(app, i)).collect()
}

/// The shown document's name, as its tab has it (the window title and the close question).
pub fn active_name(app: &App) -> String {
    info(app, app.tabs.active).map_or_else(|| tr!("Untitled").to_string(), |t| t.name)
}

/// Whether any open document has unsaved changes.
pub fn any_dirty(app: &App) -> bool {
    (0..app.tabs.len()).any(|i| session(app, i).is_some_and(Session::is_dirty))
}

/// The tabs with unsaved changes, in the bar's order.
fn dirty_ids(app: &App) -> Vec<u64> {
    infos(app).into_iter().filter(|t| t.dirty).map(|t| t.id).collect()
}

/// The recovery sessions of the open documents (they are not "left behind").
pub fn recovery_sessions(app: &App) -> Vec<String> {
    let parked = app.tabs.slots.iter().filter_map(|s| s.parked.as_ref()).filter_map(|p| p.saver.id());
    app.recovery.doc.id().into_iter().chain(parked).collect()
}

/// Ends every document's autosave (the window is closing): a clean end removes the copies,
/// otherwise each document with unsaved changes is written once more for the next start.
pub fn finish_all(app: &mut App, clean: bool) {
    app.recovery.doc.finish(&app.s, clean);
    for p in app.tabs.slots.iter_mut().filter_map(|s| s.parked.as_mut()) {
        p.saver.finish(&p.s, clean);
    }
}

/// Why another document can't be shown right now, if it can't: something is open that belongs
/// to this one and has to be finished first.
fn busy(app: &App) -> Option<&'static str> {
    if app.modal_open() && !app.tabs.asking() {
        Some(tr!("Finish with the open dialog first."))
    } else if app.publish.running() || app.plugin_export.is_some() {
        Some(tr!("Wait for the export to finish first."))
    } else {
        None
    }
}

/// Finishes what is in progress on the shown document, before it is put aside or closed: text
/// being typed and a pen path are kept, playback stops, an open drag ends its undo step.
fn settle(app: &mut App) {
    if app.playing {
        app.toggle_play();
    }
    if app.text_edit.is_some() {
        crate::stage::commit_text(app);
    }
    if app.pen.is_some() {
        crate::pathtools::commit_pen(app, false);
    }
    crate::audio_keys::cancel(app);
    if app.s.in_gesture() {
        app.s.end_gesture();
    }
    app.drag = Drag::None;
    app.tl_drag = crate::timeline::TlDrag::None;
    app.tl_layer_drag = None;
    app.me_drag = None;
    app.motion_drag = None;
    app.new_guide = None;
    app.snap_ring = None;
    app.hint_menu = None;
    app.path_pick = None;
    app.zoom_text = None;
    app.cursor_stage = None;
    app.pencil.clear();
    app.brush_samples.clear();
    app.raw_points = Value::Null;
    app.raw_touched.clear();
    app.raw_with.clear();
    // Windows that edit one keyframe or layer of this document.
    app.envelope = None;
    app.ease = None;
    app.layer_props = None;
    app.aids_dialogs = Default::default();
    // The History panel's picks and Save As Command number this document's undo steps: in
    // another document the same numbers are other steps.
    crate::macros_ui::clear_picks(app);
    app.macros.save = None;
}

/// The document in `app` is another one from here on: nothing drawn for the last one is kept
/// (ADR 0112: a document that isn't shown holds no render caches).
fn arrived(app: &mut App) {
    app.stage_cache.forget_document();
    app.last_revision = 0;
    app.heard = None;
    app.tl_heard = None;
    app.stage_nav = Default::default();
    app.tabs.reveal = true;
    app.decode_sounds();
}

/// Shows tab `to`. `false` when it can't be (a dialog of the shown document is open).
pub fn activate(app: &mut App, to: usize) -> bool {
    let from = app.tabs.active;
    if to == from {
        return true;
    }
    if to >= app.tabs.len() {
        return false;
    }
    if let Some(why) = busy(app) {
        app.status_err(why);
        return false;
    }
    settle(app);
    let Some(mut p) = app.tabs.slots.get_mut(to).and_then(|s| s.parked.take()) else { return false };
    // Unsaved work is copied before it is put aside: nothing looks after it while it waits.
    if app.prefs.autosave {
        app.recovery.doc.write_now(&app.s);
    }
    p.view.swap(app);
    std::mem::swap(&mut app.s, &mut p.s);
    std::mem::swap(&mut app.recovery.doc, &mut p.saver);
    app.s.take_shared(&mut p.s);
    p.s.drop_caches();
    if let Some(slot) = app.tabs.slots.get_mut(from) {
        slot.parked = Some(p);
    }
    app.tabs.active = to;
    arrived(app);
    true
}

/// Shows the tab with this id.
pub fn activate_id(app: &mut App, id: u64) -> bool {
    app.tabs.index_of(id).is_some_and(|i| activate(app, i))
}

/// The next tab (or the one before), round the ends (Ctrl+Tab, Ctrl+PageDown).
pub fn step(app: &mut App, forward: bool) {
    let n = app.tabs.len();
    if n < 2 {
        return;
    }
    let to = if forward { (app.tabs.active + 1) % n } else { (app.tabs.active + n - 1) % n };
    if activate(app, to) {
        app.status_ok(tr!("{name} ({number} of {count})", name = active_name(app), number = to + 1, count = n));
    }
}

/// Commands that bring a document: each opens in a tab of its own.
pub fn opens_document(id: &str) -> bool {
    matches!(id, "file.new" | "file.new_sample" | "file.new_from_template" | "file.open" | "file.open_recent" | "file.recover")
}

/// Whether two paths name the same file (as far as the file system can tell).
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if platform::WEB {
        return false;
    }
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// The tab that already has the file `id` would open.
fn already_open(app: &App, id: &str, params: &Value) -> Option<usize> {
    // A restored copy is new work, whatever file it came from.
    if params.get("unsaved") == Some(&json!(true)) {
        return None;
    }
    let path = match id {
        "file.open" => PathBuf::from(params.get("path")?.as_str()?),
        "file.open_recent" => match (params.get("index").and_then(Value::as_u64), params.get("path").and_then(Value::as_str)) {
            (Some(i), _) => app.s.recent.entries().get(usize::try_from(i).ok()?)?.path.clone(),
            (None, Some(p)) => PathBuf::from(p),
            _ => return None,
        },
        _ => return None,
    };
    (0..app.tabs.len()).find(|i| session(app, *i).and_then(|s| s.path.as_deref()).is_some_and(|p| same_file(p, &path)))
}

/// The shown document is the one the app started with, untouched: what is opened takes its
/// place.
fn gives_way(app: &App) -> bool {
    app.tabs.slots.get(app.tabs.active).is_some_and(|s| s.placeholder) && app.s.path.is_none() && !app.s.is_dirty() && app.s.history_len() == 0
}

/// Runs a command that brings a document (New, Open, a recent file, a template, a restored
/// copy) so that it arrives in a tab of its own; a file that is already open is shown instead.
/// When the command fails, the tab made for it goes again and the document that was shown is
/// back.
pub fn open(app: &mut App, id: &str, params: Value) -> Option<Value> {
    if let Some(i) = already_open(app, id, &params) {
        if activate(app, i) {
            app.start = None;
            app.status_ok(tr!("“{name}” is already open.", name = active_name(app)));
        }
        return None;
    }
    if gives_way(app) {
        let v = app.perform_here(id, params)?;
        if let Some(slot) = app.tabs.slots.get_mut(app.tabs.active) {
            slot.placeholder = false;
        }
        arrived(app);
        return Some(v);
    }
    if app.tabs.len() >= MAX_TABS {
        app.status_err(trn!("{n} document is open, the most at once. Close one to open another.", "{n} documents are open, the most at once. Close one to open another.", MAX_TABS));
        return None;
    }
    if let Some(why) = busy(app) {
        app.status_err(why);
        return None;
    }
    let from = app.tabs.slots.get(app.tabs.active).map(|s| s.id);
    let id_new = app.tabs.next_id;
    app.tabs.next_id += 1;
    // The lowest number no open document without a file has.
    let taken: Vec<u32> = (0..app.tabs.len()).filter(|i| session(app, *i).is_some_and(|s| s.path.is_none())).filter_map(|i| app.tabs.slots.get(i).map(|s| s.untitled)).collect();
    let untitled = (1..=MAX_TABS as u32 + 1).find(|n| !taken.contains(n)).unwrap_or(1);
    let parked = Parked { s: Session::new(Document::default()), view: DocView::fresh(app), saver: app.recovery.for_new_document() };
    app.tabs.slots.push(Slot { id: id_new, untitled, placeholder: false, parked: Some(Box::new(parked)) });
    if !activate(app, app.tabs.len() - 1) {
        app.tabs.slots.pop();
        return None;
    }
    match app.perform_here(id, params) {
        Some(v) => Some(v),
        None => {
            // What went wrong stays in the status bar; the tab made for it goes.
            let status = app.status.take();
            if let Some(from) = from {
                activate_id(app, from);
            }
            remove_parked(app, id_new);
            app.status = status;
            None
        }
    }
}

/// Drops a tab that isn't shown, ending its autosave cleanly (its copy goes).
fn remove_parked(app: &mut App, id: u64) {
    let Some(i) = app.tabs.index_of(id) else { return };
    if i == app.tabs.active || app.tabs.len() < 2 {
        return;
    }
    let slot = app.tabs.slots.remove(i);
    if let Some(mut p) = slot.parked {
        p.saver.finish(&p.s, true);
    }
    if i < app.tabs.active {
        app.tabs.active -= 1;
    }
}

/// Closes tab `id` without asking (its changes were saved, or the answer was Don't Save). The
/// tab next to it shows; when it was the last one, an empty document takes its place and the
/// start screen comes up, as when the app starts. `false` when it can't be closed right now.
fn close_now(app: &mut App, id: u64) -> bool {
    let Some(i) = app.tabs.index_of(id) else { return true };
    if i != app.tabs.active {
        remove_parked(app, id);
        return true;
    }
    if app.tabs.len() > 1 {
        let to = if i + 1 < app.tabs.len() { i + 1 } else { i - 1 };
        if !activate(app, to) {
            return false;
        }
        remove_parked(app, id);
        return true;
    }
    if let Some(why) = busy(app) {
        app.status_err(why);
        return false;
    }
    settle(app);
    // The last document: the window always has one.
    app.recovery.doc.finish(&app.s, true);
    app.recovery.doc = app.recovery.for_new_document();
    app.s.replace_document(Document::default(), None);
    let mut fresh = DocView::fresh(app);
    fresh.swap(app);
    if let Some(slot) = app.tabs.slots.get_mut(i) {
        slot.untitled = 1;
        slot.placeholder = true;
    }
    arrived(app);
    if app.prefs.start_screen && app.shot.is_none() {
        crate::start::open(app);
    }
    true
}

/// File › Close, a tab's × or middle click: closes these tabs, asking about each one that has
/// unsaved changes (ADR 0004).
pub fn close(app: &mut App, ids: Vec<u64>) {
    if app.tabs.closing.is_some() || ids.is_empty() {
        return;
    }
    app.playing = false;
    app.tabs.closing = Some(CloseFlow { ids, quit: false, saving: false, asked: None });
}

/// The shown tab, the others, or all of them.
pub fn close_active(app: &mut App) {
    let ids = info(app, app.tabs.active).map(|t| t.id).into_iter().collect();
    close(app, ids);
}

pub fn close_others(app: &mut App, keep: u64) {
    let ids = infos(app).into_iter().map(|t| t.id).filter(|id| *id != keep).collect();
    close(app, ids);
}

pub fn close_all(app: &mut App) {
    let ids = infos(app).into_iter().map(|t| t.id).collect();
    close(app, ids);
}

/// The window is closing with unsaved changes somewhere: one question for all of them.
pub fn quit(app: &mut App) {
    if app.tabs.closing.as_ref().is_some_and(|f| f.quit) {
        return;
    }
    app.playing = false;
    app.tabs.closing = Some(CloseFlow { ids: dirty_ids(app), quit: true, saving: false, asked: None });
}

/// The names of the documents the close question is about.
pub fn asked_about(app: &App) -> Vec<String> {
    let Some(flow) = &app.tabs.closing else { return Vec::new() };
    let all = infos(app);
    let named = |id: &u64| all.iter().find(|t| t.id == *id && t.dirty).map(|t| t.name.clone());
    if flow.quit { flow.ids.iter().filter_map(named).collect() } else { flow.ids.first().and_then(named).into_iter().collect() }
}

/// The close question's answer.
pub fn answer(app: &mut App, a: Answer) {
    let Some(mut flow) = app.tabs.closing.take() else { return };
    match a {
        Answer::Cancel => {}
        Answer::Save => {
            flow.saving = true;
            app.tabs.closing = Some(flow);
        }
        Answer::Discard if flow.quit => {
            // Nothing is saved: the window goes (see `tick`).
            flow.ids.clear();
            app.tabs.closing = Some(flow);
        }
        Answer::Discard => {
            if let Some(id) = flow.ids.first().copied()
                && close_now(app, id)
            {
                flow.ids.remove(0);
                app.tabs.closing = Some(flow);
            }
        }
    }
}

/// Starts saving the shown document: at once when it has a file, else Save As asks for one.
fn save_shown(app: &mut App) {
    if app.s.path.is_some() {
        app.perform("file.save", json!({}));
    } else {
        app.invoke("file.save_as");
    }
}

/// Per frame: moves a close on. Tabs without unsaved changes close at once; one with changes
/// is shown and asked about; after Save, the flow waits for the save (and its Save As) and
/// goes on, or stops when the save was cancelled or failed. When every document of a quit is
/// dealt with, the window closes.
pub fn tick(app: &mut App, ctx: &egui::Context) {
    let Some(mut flow) = app.tabs.closing.take() else { return };
    loop {
        let Some(&id) = flow.ids.first() else {
            if flow.quit {
                app.allow_close = true;
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            }
            return;
        };
        let Some(i) = app.tabs.index_of(id) else {
            flow.ids.remove(0);
            continue;
        };
        if !session(app, i).is_some_and(Session::is_dirty) {
            // Saved, or never changed. A quit leaves the tab: the window is about to go.
            if !flow.quit && !close_now(app, id) {
                return;
            }
            flow.ids.remove(0);
            flow.asked = None;
            // A tab's question was for that tab: the next one with changes asks again.
            flow.saving = flow.saving && flow.quit;
            continue;
        }
        if i != app.tabs.active && !activate(app, i) {
            return;
        }
        if !flow.saving {
            // The question shows (`dialogs::close_modal`).
            break;
        }
        if flow.asked != Some(id) {
            flow.asked = Some(id);
            save_shown(app);
            continue;
        }
        if app.prompt.is_some() || app.confirm.is_some() || app.file_dialog_open() {
            // Save As is asking for a name, or whether to replace a file.
            break;
        }
        // The save was cancelled or failed: the document stays open, and so does the window.
        return;
    }
    app.tabs.closing = Some(flow);
}

/// Moves the tab at `from` to `to` in the bar; the shown document stays shown.
fn move_tab(app: &mut App, from: usize, to: usize) {
    let n = app.tabs.len();
    if from == to || from >= n || to >= n {
        return;
    }
    let shown = app.tabs.slots.get(app.tabs.active).map(|s| s.id);
    let slot = app.tabs.slots.remove(from);
    app.tabs.slots.insert(to, slot);
    if let Some(i) = shown.and_then(|id| app.tabs.index_of(id)) {
        app.tabs.active = i;
    }
}

/// What the bar asks for this frame.
enum Act {
    Show(u64),
    Close(u64),
    CloseOthers(u64),
    CloseAll,
    Reveal(PathBuf),
    Move(usize, usize),
}

/// `name` in one line no wider than `width`, cut with an ellipsis.
fn elided(p: &egui::Painter, name: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), font(Type::Body), T.text);
    job.wrap = egui::text::TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    p.layout_job(job)
}

/// What a screen reader says for a tab.
fn spoken(t: &TabInfo) -> String {
    if t.dirty { tr!("{name}, unsaved changes", name = t.name) } else { t.name.clone() }
}

/// The document tabs, above the edit bar: one per open document, with a dot while it has
/// unsaved changes and a button to close it. Click shows a document, middle click closes it,
/// a drag moves the tab, right-click has Close, Close Others, Close All and where the file is.
pub fn bar(app: &mut App, ui: &mut egui::Ui) {
    let tabs = infos(app);
    let full = ui.max_rect();
    let mut acts: Vec<Act> = Vec::new();
    // How wide each tab is: its name, within limits; all a little narrower when they don't fit.
    let natural: Vec<f32> = tabs.iter().map(|t| (elided(ui.painter(), &t.name, TAB_MAX).rect.width() + 2.0 * PAD + CLOSE_W + DOT_W).clamp(TAB_MIN, TAB_MAX)).collect();
    let wanted: f32 = natural.iter().sum();
    let shrink = if wanted > full.width() && wanted > 0.0 { (full.width() / wanted).max(TAB_MIN / TAB_MAX) } else { 1.0 };
    let widths: Vec<f32> = natural.iter().map(|w| (w * shrink).max(TAB_MIN.min(*w))).collect();
    let total: f32 = widths.iter().sum();
    let max_scroll = (total - full.width()).max(0.0);
    // The wheel moves along the tabs when they still don't fit.
    if max_scroll > 0.0 && ui.rect_contains_pointer(full) {
        let d = ui.input(|i| i.smooth_scroll_delta);
        app.tabs.scroll -= if d.x.abs() > d.y.abs() { d.x } else { d.y };
    }
    if std::mem::take(&mut app.tabs.reveal) {
        let before: f32 = widths.iter().take(app.tabs.active).sum();
        let w = widths.get(app.tabs.active).copied().unwrap_or(0.0);
        app.tabs.scroll = app.tabs.scroll.min(before).max(before + w - full.width());
    }
    app.tabs.scroll = app.tabs.scroll.clamp(0.0, max_scroll);
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let mut rects = Vec::with_capacity(tabs.len());
    ui.scope_builder(egui::UiBuilder::new().id_salt("document_tabs").max_rect(full), |ui| {
        // A tab list, for screen readers: its tabs are the open documents.
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(egui::accesskit::Role::TabList);
            node.set_label(tr!("Open documents"));
        });
        ui.set_clip_rect(full.intersect(ui.clip_rect()));
        let mut x = full.left() - app.tabs.scroll;
        for (i, t) in tabs.iter().enumerate() {
            let w = widths.get(i).copied().unwrap_or(TAB_MIN);
            let r = Rect::from_min_size(pos2(x, full.top()), vec2(w, full.height()));
            x += w;
            rects.push((t.id, r));
            let shown = r.intersect(full);
            if shown.width() <= 0.0 {
                continue;
            }
            let resp = ui.interact(shown, Id::new(("document_tab", t.id)), Sense::click_and_drag());
            let said = spoken(t);
            resp.widget_info(|| egui::WidgetInfo::selected(WidgetType::SelectableLabel, true, t.active, &said));
            ui.ctx().accesskit_node_builder(resp.id, |node| {
                node.set_role(egui::accesskit::Role::Tab);
                node.set_selected(t.active);
            });
            let close_r = Rect::from_center_size(pos2(r.right() - PAD * 0.5 - CLOSE_W * 0.5, r.center().y), vec2(CLOSE_W - 4.0, CLOSE_W - 4.0));
            let close = ui.interact(close_r.intersect(full), Id::new(("document_tab_close", t.id)), Sense::click());
            a11y::control(ui, &close, WidgetType::Button, &tr!("Close {name}", name = t.name), None);
            // The tab: the shown one joins the edit bar below it.
            let p = ui.painter();
            if t.active {
                p.rect_filled(r, theme::SQUARE, T.panel);
                crate::theme::accent_bar(p, Rect::from_min_max(r.left_top(), pos2(r.right(), r.top() + 2.0)));
            } else if resp.hovered() || close.hovered() {
                p.rect_filled(r, theme::SQUARE, T.hover);
            }
            p.vline(r.right(), r.top() + 6.0..=r.bottom() - 6.0, Stroke::new(1.0, T.border));
            let mut left = r.left() + PAD;
            if t.dirty {
                // Unsaved changes: a dot before the name.
                p.circle_filled(pos2(left + 3.5, r.center().y), 3.5, if t.active { T.accent } else { T.text_dim });
                left += DOT_W;
            }
            let galley = elided(p, &t.name, close_r.left() - 4.0 - left);
            let cut = galley.elided;
            p.galley_with_override_text_color(pos2(left, r.center().y - galley.rect.height() * 0.5), galley, if t.active { T.text } else { T.text_dim });
            // The close button: an ×, brighter under the pointer.
            if close.hovered() {
                p.rect_filled(close_r, T.rc(), T.hover);
            }
            let (c, d) = (close_r.center(), 4.0);
            let ink = Stroke::new(1.5, if close.hovered() || close.has_focus() { T.icon_hot } else { T.icon });
            p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], ink);
            p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], ink);
            if resp.has_focus() {
                p.rect_stroke(r, T.rc(), Stroke::new(2.0, T.accent), StrokeKind::Inside);
            }
            // What it does.
            if close.clicked() || resp.clicked_by(egui::PointerButton::Middle) {
                acts.push(Act::Close(t.id));
            } else if resp.clicked() {
                acts.push(Act::Show(t.id));
            }
            if resp.drag_started_by(egui::PointerButton::Primary) {
                app.tabs.drag = Some(t.id);
                acts.push(Act::Show(t.id));
            }
            let place = match &t.path {
                Some(path) => path.display().to_string(),
                None => tr!("Not saved yet").to_string(),
            };
            let tip = if cut { format!("{}\n{place}", t.name) } else { place };
            close.on_hover_text(tr!("Close"));
            let resp = resp.on_hover_text(tip);
            resp.context_menu(|ui| {
                let item = |ui: &mut egui::Ui, on: bool, label: &str| {
                    let hit = ui.add_enabled(on, egui::Button::new(label)).clicked();
                    if hit {
                        ui.close();
                    }
                    hit
                };
                if item(ui, true, tr!("Close")) {
                    acts.push(Act::Close(t.id));
                }
                if item(ui, tabs.len() > 1, tr!("Close Others")) {
                    acts.push(Act::CloseOthers(t.id));
                }
                if item(ui, true, tr!("Close All")) {
                    acts.push(Act::CloseAll);
                }
                ui.separator();
                // Only a file that is somewhere can be shown there.
                if item(ui, t.path.is_some() && !platform::WEB, platform::reveal_label())
                    && let Some(path) = &t.path
                {
                    acts.push(Act::Reveal(path.clone()));
                }
            });
        }
        // A dragged tab changes places with a neighbour once the pointer is past its middle.
        if let Some(id) = app.tabs.drag {
            if !ui.input(|i| i.pointer.primary_down()) {
                app.tabs.drag = None;
            } else if let (Some(at), Some(from)) = (pointer, tabs.iter().position(|t| t.id == id)) {
                let mid = |i: usize| rects.get(i).map(|(_, r): &(u64, Rect)| r.center().x);
                if from > 0 && mid(from - 1).is_some_and(|m| at.x < m) {
                    acts.push(Act::Move(from, from - 1));
                } else if mid(from + 1).is_some_and(|m| at.x > m) {
                    acts.push(Act::Move(from, from + 1));
                }
            }
        }
    });
    app.tabs.rects = rects;
    for act in acts {
        match act {
            Act::Show(id) => {
                activate_id(app, id);
            }
            Act::Close(id) => close(app, vec![id]),
            Act::CloseOthers(id) => close_others(app, id),
            Act::CloseAll => close_all(app),
            Act::Move(from, to) => move_tab(app, from, to),
            Act::Reveal(path) => match platform::reveal_in_folder(&path) {
                Ok(()) => app.status_ok(tr!("Shown in your system's file viewer")),
                Err(e) => app.status_err(e),
            },
        }
    }
}

/// `--open tabs` (screenshots and the guide): three documents open, the shown one with unsaved
/// changes.
pub fn sample(app: &mut App) {
    let name = |app: &mut App, file: &str| {
        app.s.path = Some(PathBuf::from(file));
        app.s.mark_saved();
    };
    app.tabs.keep_first();
    name(app, "walk-cycle.anicraft");
    app.perform("file.new", crate::start::new_params(1280, 720, 24.0));
    name(app, "title-card.anicraft");
    app.perform("file.new_sample", json!({}));
    app.run("draw.rect", json!({"x": 420, "y": 250, "w": 160, "h": 100, "fill": "#f2b134", "stroke": null}));
    let _ = app.s.execute("edit.deselect", &json!({}));
    app.status_ok(tr!("Three documents are open: click a tab to show it, or press {key}.", key = platform::shortcut_label("Ctrl+Tab")));
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;
