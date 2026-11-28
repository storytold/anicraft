//! The Convert to Symbol dialog (F8, ADR 0053) and the symbol-conversion confirmation for Create
//! Motion Tween (#604). Both share the symbol type choices and remember the type last used; what
//! they change goes through `modify.convert_to_symbol` or `timeline.create_motion_tween`.

use crate::a11y::Named;
use crate::app::App;
use crate::theme::{FORM_LABEL_DOCK, T, Type, form_row, text};
use anicraft_engine::doc::{Document, FolderId, LayerId, LibraryEntry, SymbolKind};
use egui::Key;
use serde_json::json;

/// Why the dialog was opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Modify › Convert to Symbol: name, type and Library folder.
    Symbol,
    /// Insert › Create Motion Tween on artwork that must become a symbol first (#604).
    MotionTween { layer: LayerId, frame: u32 },
}

/// The dialog while it is open.
pub struct ConvertDialog {
    pub purpose: Purpose,
    pub name: String,
    pub kind: SymbolKind,
    pub folder: Option<FolderId>,
    /// The first control takes the keyboard on the first frame.
    focus: bool,
}

/// The symbol types in the order every Type list offers them: Animate's, with Movie Clip first
/// (#419).
pub const KINDS: [SymbolKind; 3] = [SymbolKind::MovieClip, SymbolKind::Button, SymbolKind::Graphic];

/// What the last conversion chose (kept for the next one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Last {
    pub folder: Option<FolderId>,
    pub kind: SymbolKind,
}

impl Default for Last {
    /// The first conversion offers a movie clip at the top level, as Animate does (#419). The
    /// command's own default stays a graphic, so scripts keep making what they made.
    fn default() -> Self {
        Last { folder: None, kind: SymbolKind::MovieClip }
    }
}

/// The folder to offer: the one used last, while it is still in the Library.
pub fn remembered(doc: &Document, last: Last) -> Option<FolderId> {
    last.folder.filter(|f| doc.folder(*f).is_some())
}

/// Opens the dialog for the selection, with the next free "Symbol n" in the remembered folder.
pub fn open(app: &mut App) {
    if app.s.selection.is_empty() {
        // The engine says why ("select something to convert").
        app.request("modify.convert_to_symbol", json!({}));
        return;
    }
    app.playing = false;
    let folder = remembered(&app.s.doc, app.convert_last);
    let name = app.s.doc.numbered_name(folder, "Symbol", app.s.doc.library.len() + 1); // i18n-exempt: a new symbol's default name is document text, as the engine gives it
    app.convert = Some(ConvertDialog { purpose: Purpose::Symbol, name, kind: app.convert_last.kind, folder, focus: true });
}

/// Opens the confirmation dialog when Create Motion Tween needs to convert the keyframe's artwork
/// into a symbol; otherwise runs `timeline.create_motion_tween` directly (#604).
pub fn open_for_motion_tween(app: &mut App) {
    if !anicraft_engine::cmds_motion::needs_symbol_conversion(&app.s, None, None) {
        app.request("timeline.create_motion_tween", json!({}));
        return;
    }
    app.playing = false;
    app.convert = Some(ConvertDialog {
        purpose: Purpose::MotionTween { layer: app.s.layer, frame: app.s.frame },
        name: String::new(),
        kind: app.convert_last.kind,
        folder: None,
        focus: true,
    });
}

pub(crate) fn kind_key(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::Graphic => "graphic",
        SymbolKind::MovieClip => "movie_clip",
        SymbolKind::Button => "button",
    }
}

/// Where `folder` is, as the dialog names it.
pub(crate) fn folder_label(doc: &Document, folder: Option<FolderId>) -> String {
    match folder {
        Some(f) => doc.entry_path(LibraryEntry::Folder(f)),
        None => tr!("Library (top level)").to_string(),
    }
}

/// The symbol type picker shared by Convert to Symbol and the Create Motion Tween confirmation.
fn kind_row(ui: &mut egui::Ui, kind: &mut SymbolKind) -> egui::Response {
    let mut resp = None;
    form_row(ui, FORM_LABEL_DOCK, tr!("Type"), |ui| {
        let r = egui::ComboBox::from_id_salt("convert_kind")
            .width(200.0)
            .selected_text(crate::panels::symbol_kind_label(*kind))
            .show_ui(ui, |ui| {
                for k in KINDS {
                    if ui.selectable_label(*kind == k, crate::panels::symbol_kind_label(k)).clicked() {
                        *kind = k;
                    }
                }
            })
            .response;
        r.set_name(tr!("Symbol type"));
        resp = Some(r);
    });
    resp.unwrap_or_else(|| ui.response())
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(d) = &mut app.convert else { return };
    let (mut submit, mut cancel) = (false, false);
    match d.purpose {
        Purpose::Symbol => {
            let doc = &app.s.doc;
            if d.folder.is_some_and(|f| doc.folder(f).is_none()) {
                d.folder = None;
            }
            let mut folders: Vec<(FolderId, String)> = doc.library_tree.folders.iter().map(|f| (f.id, doc.entry_path(LibraryEntry::Folder(f.id)))).collect();
            folders.sort_by_key(|(_, p)| p.to_lowercase());
            let title = tr!("Convert to Symbol");
            let shown = crate::windows::new(ctx, title).id(egui::Id::unique("convert_symbol")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
                ui.set_min_width(380.0);
                form_row(ui, FORM_LABEL_DOCK, tr!("Name"), |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY)).named(tr!("Symbol name"));
                    if d.focus {
                        crate::theme::focus_selected(ui, &r, &d.name);
                        d.focus = false;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        submit = true;
                    }
                });
                kind_row(ui, &mut d.kind);
                form_row(ui, FORM_LABEL_DOCK, tr!("Folder"), |ui| {
                    egui::ComboBox::from_id_salt("convert_folder")
                        .width(200.0)
                        .selected_text(folder_label(doc, d.folder))
                        .show_ui(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                                if ui.selectable_label(d.folder.is_none(), tr!("Library (top level)")).clicked() {
                                    d.folder = None;
                                }
                                for (f, path) in &folders {
                                    if ui.selectable_label(d.folder == Some(*f), path.as_str()).clicked() {
                                        d.folder = Some(*f);
                                    }
                                }
                            });
                        })
                        .response
                        .on_hover_text(tr!("The Library folder the new symbol goes into (the next conversion offers it again)"))
                        .set_name(tr!("Library folder"));
                });
                // A name the folder already has gets a number, as the engine gives it.
                let wanted = d.name.trim();
                if !wanted.is_empty() && doc.name_taken(d.folder, wanted, None) {
                    let becomes = doc.unique_name(d.folder, wanted);
                    ui.label(text(Type::Caption, tr!("{folder} already has “{name}”; the new symbol will be “{becomes}”.", folder = folder_label(doc, d.folder), name = wanted, becomes = becomes)));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(tr!("OK")).clicked() {
                        submit = true;
                    }
                    if ui.button(tr!("Cancel")).clicked() || (!egui::Popup::is_any_open(ctx) && ui.input(|i| i.key_pressed(Key::Escape))) {
                        cancel = true;
                    }
                });
            });
            crate::a11y::name_title_bar(&shown, title);
        }
        Purpose::MotionTween { .. } => {
            let title = tr!("Create Motion Tween");
            let shown = crate::windows::new(ctx, title).id(egui::Id::unique("convert_symbol")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.set_max_width(420.0);
                ui.label(text(Type::Body, tr!("Motion tweens animate a symbol. Convert this artwork to a symbol and create the tween?")));
                ui.add_space(6.0);
                let kind_resp = kind_row(ui, &mut d.kind);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let convert_btn = ui.add(egui::Button::new(text(Type::Body, tr!("Convert")).strong().color(T.on_fill)).fill(T.accent).min_size(egui::vec2(96.0, crate::theme::TARGET + 4.0)));
                    if d.focus {
                        convert_btn.request_focus();
                        d.focus = false;
                    }
                    if convert_btn.clicked() {
                        submit = true;
                    }
                    let cancel_btn = ui.add(egui::Button::new(tr!("Cancel")).min_size(egui::vec2(96.0, crate::theme::TARGET + 4.0)));
                    if cancel_btn.clicked() {
                        cancel = true;
                    }
                    if !egui::Popup::is_any_open(ctx) {
                        if ui.input(|i| i.key_pressed(Key::Escape)) {
                            cancel = true;
                        } else if !cancel_btn.has_focus() && !kind_resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            submit = true;
                        }
                    }
                });
                ui.add_space(2.0);
                ui.label(text(Type::Caption, tr!("{enter} converts · {esc} cancels", enter = crate::platform::shortcut_label("Enter"), esc = crate::platform::shortcut_label("Esc"))));
            });
            crate::a11y::name_title_bar(&shown, title);
        }
    }
    if submit {
        accept(app);
    } else if cancel {
        let was_motion = app.convert.as_ref().is_some_and(|d| matches!(d.purpose, Purpose::MotionTween { .. }));
        app.convert = None;
        if was_motion {
            app.status_ok(tr!("Cancelled"));
        }
    }
}

/// OK / Convert: runs the command with the dialog's choices and remembers them for next time.
fn accept(app: &mut App) {
    let Some(d) = app.convert.take() else { return };
    match d.purpose {
        Purpose::Symbol => {
            let name = d.name.trim();
            let mut params = json!({"kind": kind_key(d.kind), "folder": d.folder.map(|f| f.0)});
            if !name.is_empty() {
                params["name"] = json!(name);
            }
            app.convert_last = Last { folder: d.folder, kind: d.kind };
            if let Some(f) = d.folder {
                app.library.expanded.extend(app.s.doc.folder_chain(f));
            }
            app.request("modify.convert_to_symbol", params);
        }
        Purpose::MotionTween { layer, frame } => {
            app.convert_last.kind = d.kind;
            app.request("timeline.create_motion_tween", json!({"layer": layer.0, "frame": frame, "kind": kind_key(d.kind)}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::sample;

    #[test]
    fn the_remembered_folder_is_offered_while_it_exists() {
        let mut s = Session::new(sample::demo());
        let f = FolderId(s.execute("library.new_folder", &json!({"name": "Props"})).unwrap()["folder"].as_u64().unwrap());
        let last = Last { folder: Some(f), kind: SymbolKind::MovieClip };
        assert_eq!(remembered(&s.doc, last), Some(f));
        s.execute("library.delete", &json!({"items": [{"folder": f.0}]})).unwrap();
        assert_eq!(remembered(&s.doc, last), None, "a deleted folder falls back to the top level");
        assert_eq!(remembered(&s.doc, Last::default()), None);
    }

    #[test]
    fn ok_converts_into_the_folder_and_the_next_dialog_offers_it_again() {
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, Session::new(sample::demo()), Some(std::path::PathBuf::from("unused.png")));
        // Nothing selected: no dialog, the engine says why.
        app.run("edit.deselect", json!({}));
        open(&mut app);
        assert!(app.convert.is_none());
        let f = FolderId(app.run("library.new_folder", json!({"name": "Props"})).unwrap()["folder"].as_u64().unwrap());
        app.run("draw.rect", json!({"x": 10, "y": 10, "w": 20, "h": 20, "object": true}));
        open(&mut app);
        let d = app.convert.as_mut().expect("the dialog opens");
        assert_eq!(d.folder, None, "the top level the first time");
        assert_eq!(d.kind, SymbolKind::MovieClip, "a movie clip the first time, as in Animate (#419)");
        assert_eq!(KINDS, [SymbolKind::MovieClip, SymbolKind::Button, SymbolKind::Graphic], "Animate's order");
        assert!(d.name.starts_with("Symbol "));
        d.folder = Some(f);
        d.kind = SymbolKind::Button;
        d.name = "  Lamp ".into();
        accept(&mut app);
        assert!(app.convert.is_none());
        let sym = app.s.doc.library.iter().find(|s| s.name == "Lamp").expect("converted, the name trimmed");
        assert_eq!(sym.kind, SymbolKind::Button);
        assert_eq!(app.s.doc.parent_of(LibraryEntry::Item(anicraft_engine::doc::LibraryItem::Symbol(sym.id))), Some(f));
        assert!(app.library.expanded.contains(&f), "the folder opens in the Library");
        // The next conversion offers Props and Button again; an empty name takes the default.
        app.run("draw.rect", json!({"x": 60, "y": 10, "w": 20, "h": 20, "object": true}));
        open(&mut app);
        let d = app.convert.as_mut().expect("the dialog opens");
        assert_eq!((d.folder, d.kind), (Some(f), SymbolKind::Button));
        d.name = String::new();
        accept(&mut app);
        assert_eq!(app.s.doc.library_children(Some(f)).len(), 2);
        // The dialog draws (the clash note too) without the document changing.
        app.run("draw.rect", json!({"x": 90, "y": 10, "w": 20, "h": 20, "object": true}));
        open(&mut app);
        if let Some(d) = app.convert.as_mut() {
            d.name = "Lamp".into();
        }
        let before = app.s.doc.clone();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.convert.is_some());
        assert_eq!(app.s.doc, before);
    }

    #[test]
    fn motion_tween_asks_before_converting_and_cancel_leaves_document_unchanged() {
        let ctx = egui::Context::default();
        let mut app = App::with_context(&ctx, Session::default(), Some(std::path::PathBuf::from("unused.png")));
        app.run("draw.rect", json!({"x": 10, "y": 10, "w": 40, "h": 30, "object": false}));
        let before = app.s.doc.clone();
        let steps = app.s.history_len();
        let symbols_before = app.s.doc.library.len();

        // Invoking Create Motion Tween on raw artwork opens the confirmation dialog first.
        app.invoke("timeline.create_motion_tween");
        let d = app.convert.as_ref().expect("confirmation dialog opens for raw artwork");
        assert!(matches!(d.purpose, Purpose::MotionTween { .. }));
        assert_eq!(d.kind, SymbolKind::MovieClip, "reuses Convert to Symbol's default");
        assert!(app.modal_open());
        assert_eq!(app.s.doc, before, "opening the dialog does not mutate the document");

        // Drawing the dialog does not mutate the document.
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.convert.is_some());
        assert_eq!(app.s.doc, before);

        // Pressing Escape cancels without changing the document or adding an undo step.
        let esc = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut out = ctx.run_ui(esc, |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.convert.is_none(), "Escape closes the confirmation dialog");
        assert_eq!(app.s.doc, before, "Cancel leaves the document unchanged");
        assert_eq!(app.s.history_len(), steps, "Cancel records no undo step");
        assert_eq!(app.s.doc.library.len(), symbols_before);

        // Re-opening, choosing Graphic and confirming converts to Graphic + creates the tween in one undo step.
        app.invoke("timeline.create_motion_tween");
        let d = app.convert.as_mut().expect("dialog re-opens");
        d.kind = SymbolKind::Graphic;
        accept(&mut app);
        assert!(app.convert.is_none());
        assert_eq!(app.convert_last.kind, SymbolKind::Graphic, "remembers the chosen symbol type");
        assert_eq!(app.s.doc.library.len(), symbols_before + 1);
        assert_eq!(app.s.doc.library.last().map(|s| s.kind), Some(SymbolKind::Graphic));
        assert!(app.s.active_layer().and_then(|l| l.keyframes.first()).is_some_and(|k| k.motion.is_some()));
        assert_eq!(app.s.history_len(), steps + 1, "one undo step");
        app.run("edit.undo", json!({}));
        assert_eq!(app.s.doc, before);

        // Already a single symbol instance: creates the tween directly with no dialog.
        app.run("edit.select_all", json!({}));
        app.run("modify.convert_to_symbol", json!({"name": "Hero", "kind": "movie_clip"}));
        let lib_count = app.s.doc.library.len();
        app.invoke("timeline.create_motion_tween");
        assert!(app.convert.is_none(), "no dialog for an existing symbol instance");
        assert_eq!(app.s.doc.library.len(), lib_count, "no new symbol created");
        assert!(app.s.active_layer().and_then(|l| l.keyframes.first()).is_some_and(|k| k.motion.is_some()));
    }
}
