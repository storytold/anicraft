//! The Create New Symbol dialog (Ctrl+F8, #655): creates an empty symbol in the Library with a
//! chosen name, type and folder, and opens it for editing via `library.new_symbol`.

use crate::a11y::Named;
use crate::app::App;
use crate::theme::{FORM_LABEL_DOCK, T, Type, form_row, text};
use anicraft_engine::doc::{Document, FolderId, LibraryEntry, LibraryItem, SymbolId, SymbolKind};
use egui::Key;
use serde_json::json;

/// The dialog while it is open.
pub struct NewSymbolDialog {
    pub name: String,
    pub kind: SymbolKind,
    pub folder: Option<FolderId>,
    pub error: Option<String>,
    /// The name field takes the keyboard on the first frame.
    focus: bool,
}

const KINDS: [SymbolKind; 3] = [SymbolKind::Graphic, SymbolKind::MovieClip, SymbolKind::Button];

/// Opens the dialog at the top level of the Library with the next free "Symbol n".
pub fn open(app: &mut App) {
    open_in(app, None);
}

/// Opens the dialog targeting `folder`, prefilled with the next free "Symbol n" there.
pub fn open_in(app: &mut App, folder: Option<FolderId>) {
    app.playing = false;
    let folder = folder.filter(|f| app.s.doc.folder(*f).is_some());
    let name = app.s.doc.numbered_name(folder, "Symbol", app.s.doc.library.len() + 1); // i18n-exempt: a new symbol's default name is document text, as the engine gives it
    app.new_symbol = Some(NewSymbolDialog { name, kind: SymbolKind::Graphic, folder, error: None, focus: true });
}

fn kind_key(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::Graphic => "graphic",
        SymbolKind::MovieClip => "movie_clip",
        SymbolKind::Button => "button",
    }
}

fn folder_label(doc: &Document, folder: Option<FolderId>) -> String {
    match folder {
        Some(f) => doc.entry_path(LibraryEntry::Folder(f)),
        None => tr!("Library (top level)").to_string(),
    }
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(d) = &mut app.new_symbol else { return };
    let doc = &app.s.doc;
    if d.folder.is_some_and(|f| doc.folder(f).is_none()) {
        d.folder = None;
    }
    let mut folders: Vec<(FolderId, String)> = doc.library_tree.folders.iter().map(|f| (f.id, doc.entry_path(LibraryEntry::Folder(f.id)))).collect();
    folders.sort_by_key(|(_, p)| p.to_lowercase());
    let (mut submit, mut cancel) = (false, false);
    let title = tr!("Create New Symbol");
    let shown = crate::windows::new(ctx, title).id(egui::Id::unique("new_symbol")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
        ui.set_min_width(360.0);
        form_row(ui, FORM_LABEL_DOCK, tr!("Name"), |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY)).named(tr!("Name"));
            if d.focus {
                crate::theme::focus_selected(ui, &r, &d.name);
                d.focus = false;
            }
            if r.changed() {
                d.error = None;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
            }
        });
        form_row(ui, FORM_LABEL_DOCK, tr!("Type"), |ui| {
            egui::ComboBox::from_id_salt("new_symbol_kind")
                .width(200.0)
                .selected_text(crate::panels::symbol_kind_label(d.kind))
                .show_ui(ui, |ui| {
                    for k in KINDS {
                        if ui.selectable_label(d.kind == k, crate::panels::symbol_kind_label(k)).clicked() {
                            d.kind = k;
                        }
                    }
                })
                .response
                .set_name(tr!("Type"));
        });
        form_row(ui, FORM_LABEL_DOCK, tr!("Folder"), |ui| {
            egui::ComboBox::from_id_salt("new_symbol_folder")
                .width(200.0)
                .selected_text(folder_label(doc, d.folder))
                .show_ui(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        if ui.selectable_label(d.folder.is_none(), tr!("Library (top level)")).clicked() {
                            d.folder = None;
                            d.error = None;
                        }
                        for (f, path) in &folders {
                            if ui.selectable_label(d.folder == Some(*f), path.as_str()).clicked() {
                                d.folder = Some(*f);
                                d.error = None;
                            }
                        }
                    });
                })
                .response
                .on_hover_text(tr!("The Library folder the new symbol goes into"))
                .set_name(tr!("Folder"));
        });
        if let Some(err) = &d.error {
            ui.label(text(Type::Caption, err).color(T.error));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tr!("OK")).clicked() {
                submit = true;
            }
            if ui.button(tr!("Cancel")).clicked() {
                cancel = true;
            }
        });
    });
    crate::a11y::name_title_bar(&shown, title);
    if !egui::Popup::is_any_open(ctx) {
        if !cancel && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            cancel = true;
        } else if !submit && !cancel && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
            submit = true;
        }
    }
    if submit {
        accept(app);
    } else if cancel {
        app.new_symbol = None;
    }
}

/// OK: runs `library.new_symbol` and closes the dialog on success; on refusal keeps it open with
/// the error shown.
fn accept(app: &mut App) {
    let Some((name, kind, folder)) = app.new_symbol.as_ref().map(|d| (d.name.clone(), d.kind, d.folder)) else { return };
    let params = json!({"name": name, "kind": kind_key(kind), "folder": folder.map(|f| f.0), "edit": true});
    if let Some(v) = app.run("library.new_symbol", params) {
        app.new_symbol = None;
        if let Some(f) = folder {
            app.library.expanded.extend(app.s.doc.folder_chain(f));
        }
        if let Some(id) = v["id"].as_u64() {
            let item = LibraryItem::Symbol(SymbolId(id));
            app.library_sel = Some(item);
            app.library.sel = vec![LibraryEntry::Item(item)];
        }
        let created = v["name"].as_str().unwrap_or("");
        app.status_ok(tr!("Created symbol “{name}” — {undo} to undo", name = created, undo = app.undo_key()));
    } else if let Some(d) = app.new_symbol.as_mut() {
        d.error = app.status.as_ref().map(|s| s.text.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::Session;
    use anicraft_engine::doc::sample;

    fn app(ctx: &egui::Context) -> App {
        App::with_context(ctx, Session::new(sample::demo()), Some(std::path::PathBuf::from("unused.png")))
    }

    #[test]
    fn menu_and_shortcut_open_the_dialog_and_ok_creates_the_symbol() {
        let spec = anicraft_engine::find_command("library.new_symbol").expect("command registered");
        assert_eq!((spec.menu, spec.shortcut), ("Insert", Some("Ctrl+F8")));

        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        let before = app.s.doc.library.len();
        // Ctrl+F8 opens the dialog.
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
            events: vec![egui::Event::Key { key: Key::F8, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }],
            ..Default::default()
        };
        let mut out = ctx.run_ui(input.clone(), |ui| app.frame(ui));
        out.textures_delta.clear();
        let d = app.new_symbol.as_ref().expect("Ctrl+F8 opens the dialog");
        assert!(app.modal_open());
        assert_eq!(d.kind, SymbolKind::Graphic);
        assert!(d.name.starts_with("Symbol "));

        // Escape closes it without changing the document.
        input.events = vec![egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }];
        let mut out = ctx.run_ui(input, |ui| window(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.new_symbol.is_none());
        assert_eq!(app.s.doc.library.len(), before);

        // Menu action opens it; an invalid name is refused and keeps the dialog open with an error.
        app.invoke("library.new_symbol");
        if let Some(d) = app.new_symbol.as_mut() {
            d.name = "   ".into();
        }
        accept(&mut app);
        assert!(app.new_symbol.as_ref().is_some_and(|d| d.error.is_some()));
        assert_eq!(app.s.doc.library.len(), before);

        // OK with a valid name and type creates the empty symbol and enters it.
        if let Some(d) = app.new_symbol.as_mut() {
            d.name = "Hero".into();
            d.kind = SymbolKind::Button;
        }
        accept(&mut app);
        assert!(app.new_symbol.is_none());
        assert_eq!(app.s.doc.library.len(), before + 1);
        let sym = app.s.doc.library.iter().find(|s| s.name == "Hero").expect("created");
        assert_eq!(sym.kind, SymbolKind::Button);
        assert_eq!(app.s.editing_symbol(), Some(sym.id));
    }
}
