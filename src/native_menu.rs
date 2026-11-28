//! macOS: the system menu bar, in place of the in-window one. Its menus are the in-window bar's
//! ([`menus::native_tree`]), item for item, with the application menu and the system's Hide,
//! Minimize, Zoom and Full Screen items Mac apps have.
//!
//! An item chosen with the mouse runs what its in-window item runs ([`menus::run_native`]). An item
//! chosen by its shortcut (AppKit takes the key from the window when a menu item has it) goes back
//! to egui as that key press, so the app's own key rules still decide: a text field keeps its keys,
//! an open dialog holds them, and the Test Movie window has its own. Only shortcuts with ⌘ go on
//! menu items; the others (`F6`, `B`, `Enter`) stay egui's and the native menu shows none.
//!
//! Each frame the menus are compared with what they show: labels, enabled and checked change in
//! place; a change of rows or shortcuts (a new recent file, a document opened, a key reassigned)
//! builds the menu bar again.

use crate::app::App;
use crate::menus::{self, Act, Row};
use muda::accelerator::{Accelerator, Code, Modifiers as Mods};
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::mpsc::{Receiver, channel};

enum Widget {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

/// An item of ours; its menu id is its index in [`NativeMenu::slots`].
struct Slot {
    widget: Widget,
    act: Act,
    keys: String,
    /// What it shows now: label, enabled, checked.
    shown: (String, bool, bool),
}

/// What makes the menu bar be built again when it changes: the rows, their shortcuts and which are
/// check items. Labels, enabled and checked are set in place.
#[derive(Debug, PartialEq)]
enum Shape {
    Item(String, bool),
    Sub,
    End,
    Sep,
}

pub struct NativeMenu {
    _menu: Menu,
    subs: Vec<(Submenu, String)>,
    slots: Vec<Slot>,
    shape: Vec<Shape>,
    events: Receiver<String>,
}

/// The application menu's own items, before the tree's: About, Settings and Quit.
fn app_rows() -> [Row; 3] {
    let row = |label: String, keys: &str, act| Row::Item { label, keys: keys.into(), enabled: true, checked: None, act };
    [
        row(tr!("About AniCraft").to_string(), "", Act::About),
        row(tr!("Settings…").to_string(), "Ctrl+,", Act::Settings),
        row(tr!("Quit AniCraft").to_string(), "Ctrl+Q", Act::Quit),
    ]
}

fn shape_of(rows: &[Row], out: &mut Vec<Shape>) {
    for r in rows {
        match r {
            Row::Item { keys, checked, .. } => out.push(Shape::Item(keys.clone(), checked.is_some())),
            Row::Sub(_, sub) => {
                out.push(Shape::Sub);
                shape_of(sub, out);
                out.push(Shape::End);
            }
            Row::Sep => out.push(Shape::Sep),
        }
    }
}

fn shape(tree: &[(&str, String, Vec<Row>)]) -> Vec<Shape> {
    let mut out = Vec::new();
    for (_, _, rows) in tree {
        out.push(Shape::Sub);
        shape_of(rows, &mut out);
        out.push(Shape::End);
    }
    out
}

/// The menu key for a chord in registry notation: one with ⌘ (`Ctrl` there), else none.
pub(crate) fn accelerator(keys: &str) -> Option<Accelerator> {
    let (m, key) = crate::keymap::parse(keys)?;
    if !m.command {
        return None;
    }
    let mut mods = Mods::META;
    if m.shift {
        mods |= Mods::SHIFT;
    }
    if m.alt {
        mods |= Mods::ALT;
    }
    Some(Accelerator::new(mods, code(key)?))
}

fn code(key: egui::Key) -> Option<Code> {
    use egui::Key as K;
    let letters = [
        Code::KeyA,
        Code::KeyB,
        Code::KeyC,
        Code::KeyD,
        Code::KeyE,
        Code::KeyF,
        Code::KeyG,
        Code::KeyH,
        Code::KeyI,
        Code::KeyJ,
        Code::KeyK,
        Code::KeyL,
        Code::KeyM,
        Code::KeyN,
        Code::KeyO,
        Code::KeyP,
        Code::KeyQ,
        Code::KeyR,
        Code::KeyS,
        Code::KeyT,
        Code::KeyU,
        Code::KeyV,
        Code::KeyW,
        Code::KeyX,
        Code::KeyY,
        Code::KeyZ,
    ];
    let digits =
        [Code::Digit0, Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4, Code::Digit5, Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9];
    let f = [
        Code::F1,
        Code::F2,
        Code::F3,
        Code::F4,
        Code::F5,
        Code::F6,
        Code::F7,
        Code::F8,
        Code::F9,
        Code::F10,
        Code::F11,
        Code::F12,
        Code::F13,
        Code::F14,
        Code::F15,
        Code::F16,
        Code::F17,
        Code::F18,
        Code::F19,
        Code::F20,
    ];
    let name = key.name();
    if name.len() == 1
        && let Some(c) = name.chars().next()
    {
        if c.is_ascii_uppercase() {
            return letters.get(usize::from(c as u8 - b'A')).copied();
        }
        if c.is_ascii_digit() {
            return digits.get(usize::from(c as u8 - b'0')).copied();
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<usize>().ok()) {
        return f.get(n.checked_sub(1)?).copied();
    }
    Some(match key {
        K::Comma => Code::Comma,
        K::Period => Code::Period,
        K::Minus => Code::Minus,
        K::Equals | K::Plus => Code::Equal,
        K::OpenBracket => Code::BracketLeft,
        K::CloseBracket => Code::BracketRight,
        K::Semicolon | K::Colon => Code::Semicolon,
        K::Quote => Code::Quote,
        K::Slash | K::Questionmark => Code::Slash,
        K::Backslash | K::Pipe => Code::Backslash,
        K::Backtick => Code::Backquote,
        K::Enter => Code::Enter,
        K::Tab => Code::Tab,
        K::Space => Code::Space,
        K::Backspace => Code::Backspace,
        K::Delete => Code::Delete,
        K::Escape => Code::Escape,
        K::ArrowUp => Code::ArrowUp,
        K::ArrowDown => Code::ArrowDown,
        K::ArrowLeft => Code::ArrowLeft,
        K::ArrowRight => Code::ArrowRight,
        K::PageUp => Code::PageUp,
        K::PageDown => Code::PageDown,
        K::Home => Code::Home,
        K::End => Code::End,
        _ => return None,
    })
}

/// The key press an item's shortcut stands for, when the modifiers held now are the shortcut's:
/// the item was chosen from the keyboard. `None` for a click (or an item without a ⌘ shortcut).
pub(crate) fn as_key(keys: &str, held: egui::Modifiers) -> Option<(egui::Key, egui::Modifiers)> {
    accelerator(keys)?;
    let (m, key) = crate::keymap::parse(keys)?;
    (held.command && held.shift == m.shift && held.alt == m.alt)
        .then_some((key, egui::Modifiers { command: true, mac_cmd: true, shift: m.shift, alt: m.alt, ctrl: false }))
}

struct Builder<'a> {
    slots: &'a mut Vec<Slot>,
    subs: &'a mut Vec<(Submenu, String)>,
}

impl Builder<'_> {
    fn add(&mut self, parent: &Submenu, rows: &[Row]) {
        for r in rows {
            match r {
                Row::Item { label, keys, enabled, checked, act } => {
                    let id = self.slots.len().to_string();
                    let accel = accelerator(keys);
                    let widget = match checked {
                        Some(on) => Widget::Check(CheckMenuItem::with_id(id, label, *enabled, *on, accel)),
                        None => Widget::Plain(MenuItem::with_id(id, label, *enabled, accel)),
                    };
                    let _ = match &widget {
                        Widget::Plain(w) => parent.append(w),
                        Widget::Check(w) => parent.append(w),
                    };
                    self.slots.push(Slot {
                        widget,
                        act: act.clone(),
                        keys: keys.clone(),
                        shown: (label.clone(), *enabled, checked.unwrap_or(false)),
                    });
                }
                Row::Sub(label, sub) => {
                    let s = Submenu::new(label, true);
                    self.add(&s, sub);
                    let _ = parent.append(&s);
                    self.subs.push((s, label.clone()));
                }
                Row::Sep => {
                    let _ = parent.append(&PredefinedMenuItem::separator());
                }
            }
        }
    }
}

impl NativeMenu {
    /// Puts AniCraft's menu bar in place of the system's default one; its picks wake `ctx`. Call on
    /// the main thread once the window is open.
    pub fn install(ctx: &egui::Context, app: &App) -> Option<NativeMenu> {
        let (tx, events) = channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e.id().0.clone());
            ctx.request_repaint();
        }));
        Some(Self::build(app, events))
    }

    fn build(app: &App, events: Receiver<String>) -> NativeMenu {
        let tree = menus::native_tree(app);
        let menu = Menu::new();
        let (mut slots, mut subs) = (Vec::new(), Vec::new());
        let mut b = Builder { slots: &mut slots, subs: &mut subs };
        let sep = PredefinedMenuItem::separator;

        // The first submenu is the application menu; macOS titles it with the app's name.
        let app_menu = Submenu::new("AniCraft", true);
        let [about, settings, quit] = app_rows();
        b.add(&app_menu, &[about, Row::Sep, settings]);
        let _ = app_menu.append_items(&[
            &sep(),
            &PredefinedMenuItem::services(Some(tr!("Services"))),
            &sep(),
            &PredefinedMenuItem::hide(Some(tr!("Hide AniCraft"))),
            &PredefinedMenuItem::hide_others(Some(tr!("Hide Others"))),
            &PredefinedMenuItem::show_all(Some(tr!("Show All"))),
            &sep(),
        ]);
        // Not AppKit's terminate:, which would skip the close guard and its unsaved-changes question.
        b.add(&app_menu, &[quit]);
        let _ = menu.append(&app_menu);

        for (top, title, rows) in &tree {
            let sub = Submenu::new(title, true);
            if *top == "Window" {
                let _ = sub.append_items(&[
                    &PredefinedMenuItem::minimize(Some(tr!("Minimize"))),
                    &PredefinedMenuItem::maximize(Some(tr!("Zoom"))),
                    &sep(),
                ]);
            }
            b.add(&sub, rows);
            match *top {
                "View" => {
                    let _ = sub.append_items(&[&sep(), &PredefinedMenuItem::fullscreen(Some(tr!("Enter Full Screen")))]);
                }
                "Window" => {
                    let _ = sub.append_items(&[&sep(), &PredefinedMenuItem::bring_all_to_front(Some(tr!("Bring All to Front")))]);
                }
                _ => {}
            }
            let _ = menu.append(&sub);
            // macOS adds its search field to the Help menu.
            if *top == "Help" {
                sub.set_as_help_menu_for_nsapp();
            }
            b.subs.push((sub, title.clone()));
        }
        menu.init_for_nsapp();
        NativeMenu { _menu: menu, subs, slots, shape: shape(&tree), events }
    }

    /// Runs the items chosen since the last frame. Call before the frame reads its keys.
    pub fn poll(&mut self, app: &mut App, ctx: &egui::Context) {
        let held = ctx.input(|i| i.modifiers);
        let text = crate::a11y::keyboard(ctx) == crate::a11y::Keyboard::Text;
        while let Ok(id) = self.events.try_recv() {
            let Some(slot) = id.parse::<usize>().ok().and_then(|i| self.slots.get(i)) else { continue };
            // Greyed since the menu was drawn.
            if !slot.shown.1 {
                continue;
            }
            match &slot.act {
                // The system clipboard events, which reach a text field or the stage as the keys do.
                Act::Cmd("edit.copy") => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCopy),
                Act::Cmd("edit.cut") => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCut),
                Act::Cmd("edit.paste") => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
                act => {
                    // A click on Undo, Redo or Select All while a text field has the keys is the field's,
                    // as on a Mac.
                    let field = text && matches!(act, Act::Cmd("edit.undo" | "edit.redo" | "edit.select_all"));
                    let key = as_key(&slot.keys, held)
                        .or_else(|| field.then(|| as_key(&slot.keys, egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND)).flatten());
                    match key {
                        Some((key, modifiers)) => {
                            ctx.input_mut(|i| i.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }))
                        }
                        None => {
                            let act = act.clone();
                            menus::run_native(app, ctx, &act);
                        }
                    }
                }
            }
        }
    }

    /// Brings the menus up to date with the app, after the frame.
    pub fn sync(&mut self, app: &App) {
        let tree = menus::native_tree(app);
        if shape(&tree) != self.shape {
            let events = std::mem::replace(&mut self.events, channel().1);
            *self = Self::build(app, events);
            return;
        }
        // While a dialog holds the keys, the menus wait too; Quit still asks about unsaved work.
        let held = app.modal_open() || app.key_capture.is_some();
        let [about, settings, quit] = app_rows();
        let mut rows: Vec<&Row> = vec![&about, &settings, &quit];
        let mut titles: Vec<&str> = Vec::new();
        fn walk<'a>(rows: &'a [Row], out: &mut Vec<&'a Row>, titles: &mut Vec<&'a str>) {
            for r in rows {
                match r {
                    Row::Item { .. } => out.push(r),
                    Row::Sub(label, sub) => {
                        walk(sub, out, titles);
                        titles.push(label);
                    }
                    Row::Sep => {}
                }
            }
        }
        for (_, title, sub) in &tree {
            walk(sub, &mut rows, &mut titles);
            titles.push(title);
        }
        for (slot, row) in self.slots.iter_mut().zip(rows) {
            let Row::Item { label, enabled, checked, act, .. } = row else { continue };
            let enabled = *enabled && (!held || *act == Act::Quit);
            let now = (label.clone(), enabled, checked.unwrap_or(false));
            slot.act = act.clone();
            if slot.shown == now {
                continue;
            }
            match &slot.widget {
                Widget::Plain(w) => {
                    w.set_text(&now.0);
                    w.set_enabled(now.1);
                }
                Widget::Check(w) => {
                    w.set_text(&now.0);
                    w.set_enabled(now.1);
                    w.set_checked(now.2);
                }
            }
            slot.shown = now;
        }
        for ((sub, shown), title) in self.subs.iter_mut().zip(titles) {
            if shown != title {
                sub.set_text(title);
                *shown = title.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ⌘ shortcut an action can have is a menu key, so its item shows it.
    #[test]
    fn every_command_shortcut_is_a_menu_key() {
        for a in crate::keymap::actions() {
            for k in &a.defaults {
                if crate::keymap::parse(k).is_some_and(|(m, _)| m.command) {
                    assert!(accelerator(k).is_some(), "{k} ({})", a.id);
                }
            }
        }
        for k in ["Ctrl+=", "Ctrl+-", "Ctrl+0", "Ctrl+1", "Ctrl+K", "Ctrl+Alt+Shift+K", "Ctrl+,", "Ctrl+Q"] {
            assert!(accelerator(k).is_some(), "{k}");
        }
        assert!(accelerator("F6").is_none() && accelerator("Alt+Shift+O").is_none(), "keys without ⌘ stay egui's");
    }

    /// A shortcut's item chosen with its keys held goes back as the key press; a click does not.
    #[test]
    fn an_item_chosen_by_its_keys_is_the_key_press() {
        let cmd = egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND;
        assert_eq!(as_key("Ctrl+S", cmd).map(|(k, _)| k), Some(egui::Key::S));
        assert!(as_key("Ctrl+S", egui::Modifiers::NONE).is_none(), "a click");
        assert!(as_key("Ctrl+Shift+S", cmd).is_none(), "⌘S held is not ⇧⌘S's item");
        assert!(as_key("F6", cmd).is_none(), "no menu key");
    }
}
