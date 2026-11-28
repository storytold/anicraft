//! Text in scripts the interface's fonts lack (#482). Inter, JetBrains Mono and egui's own
//! fonts have no Japanese, Chinese, Korean, Arabic, Thai or Indic letters, so a layer, a symbol
//! or a text block named or written in one of them showed as empty boxes in the timeline, the
//! Library and the Movie Explorer, while the stage (which lays text out with the computer's
//! fonts) drew it correctly.
//!
//! The fix is a fallback, and it costs nothing until it is needed. Each frame the text that was
//! painted is checked for characters no interface font can draw. For those, a background thread
//! asks the computer's fonts (`anicraft_text::Fonts::face_covering`) for a face that has them,
//! the system's own interface faces first. What it finds is added after every other font, in
//! both families, so nothing that drew before changes. At most [`MAX_FACES`] faces of at most
//! [`MAX_FACE_BYTES`] each are kept, and each character is looked up once.

use crate::app::App;
use crate::theme::{Type, UserFont};
use std::collections::BTreeSet;

/// Most fallback faces kept (a CJK face is 10 to 30 MB in memory).
pub const MAX_FACES: usize = 6;
/// The largest font file loaded as a fallback.
pub const MAX_FACE_BYTES: usize = 48 << 20;
/// Most characters ever looked up in a run: each is looked up once, found or not.
const MAX_ASKED: usize = 4096;
/// Most characters handed to one search.
const MAX_PER_SEARCH: usize = 64;
/// How deep nested shape lists are followed when reading what was painted.
const MAX_SHAPE_DEPTH: u32 = 8;

/// Interface faces that come with each system, for the scripts the bundled fonts lack: tried
/// in this order before any other installed font. Japanese first for Han characters, unless
/// the system's language says otherwise ([`preferred`]).
// i18n-exempt: font family names
const JAPANESE: [&str; 6] = ["Yu Gothic UI", "Meiryo UI", "Meiryo", "Hiragino Sans", "Noto Sans CJK JP", "Noto Sans JP"]; // i18n-exempt: font family names
const CHINESE: [&str; 8] = ["Microsoft YaHei UI", "Microsoft YaHei", "Microsoft JhengHei UI", "PingFang SC", "PingFang TC", "Noto Sans CJK SC", "Noto Sans SC", "WenQuanYi Micro Hei"]; // i18n-exempt: font family names
const KOREAN: [&str; 4] = ["Malgun Gothic", "Apple SD Gothic Neo", "Noto Sans CJK KR", "Noto Sans KR"]; // i18n-exempt: font family names
const OTHERS: [&str; 17] = ["Segoe UI", "Nirmala UI", "Leelawadee UI", "Segoe UI Symbol", "SF Hebrew", "Arial Hebrew", "Geeza Pro", "Thonburi", "Kohinoor Devanagari", "Arial Unicode MS", "Noto Sans", "Noto Sans Hebrew", "Noto Sans Arabic", "Noto Sans Devanagari", "Noto Sans Thai", "DejaVu Sans", "Droid Sans Fallback"]; // i18n-exempt: font family names

/// The families to try first, for a system whose language is `locale` (`ja-JP`, `zh_CN`, …):
/// Han characters are drawn in the reader's own language's face when it is known.
pub fn preferred(locale: Option<&str>) -> Vec<&'static str> {
    let lang = locale.unwrap_or("").get(..2).unwrap_or("").to_ascii_lowercase();
    let (j, c, k) = (JAPANESE.as_slice(), CHINESE.as_slice(), KOREAN.as_slice());
    let cjk = match lang.as_str() {
        "zh" => [c, j, k],
        "ko" => [k, j, c],
        _ => [j, c, k],
    };
    cjk.into_iter().flatten().chain(&OTHERS).copied().collect()
}

/// The fallback faces in use, and what has been looked up.
#[derive(Default)]
pub struct Fallback {
    /// Off in tests and wherever the computer's fonts can't be read.
    pub enabled: bool,
    /// Faces added after the interface's fonts (`theme::install_fonts`).
    pub faces: Vec<UserFont>,
    /// Characters looked up already, found or not.
    asked: BTreeSet<char>,
    /// The search in progress.
    #[cfg(not(target_arch = "wasm32"))]
    rx: Option<std::sync::mpsc::Receiver<Vec<UserFont>>>,
}

/// Characters a font is never asked to draw.
fn invisible(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{E0000}'..='\u{E0FFF}')
}

/// The characters of `texts` worth looking up: not ASCII, not invisible, not asked before, each
/// once, at most `room` of them.
pub fn candidates<'a>(texts: impl IntoIterator<Item = &'a str>, asked: &BTreeSet<char>, room: usize) -> BTreeSet<char> {
    let mut out = BTreeSet::new();
    for t in texts {
        if t.is_ascii() {
            continue;
        }
        for c in t.chars().filter(|c| !c.is_ascii() && !invisible(*c) && !asked.contains(c)) {
            if out.len() >= room {
                return out;
            }
            out.insert(c);
        }
    }
    out
}

fn visit(shape: &egui::Shape, depth: u32, texts: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(t) if !t.galley.job.text.is_ascii() => texts.push(t.galley.job.text.clone()),
        egui::Shape::Vec(v) if depth < MAX_SHAPE_DEPTH => v.iter().for_each(|s| visit(s, depth + 1, texts)),
        _ => {}
    }
}

/// The text painted so far this frame that is not plain ASCII.
fn painted(ctx: &egui::Context) -> Vec<String> {
    let layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
    let mut texts = Vec::new();
    ctx.graphics(|g| {
        for id in layers {
            if let Some(list) = g.get(id) {
                list.all_entries().for_each(|c| visit(&c.shape, 0, &mut texts));
            }
        }
    });
    texts
}

/// Faces for `chars`, in the order found: one covers many characters, so a face is looked for
/// only for a character none of the faces found so far has. At most `room` faces.
#[cfg(not(target_arch = "wasm32"))]
pub fn find(fonts: &anicraft_engine::text::Fonts, chars: &[char], prefer: &[&str], room: usize) -> Vec<UserFont> {
    let mut out: Vec<UserFont> = Vec::new();
    for &c in chars {
        if out.len() >= room {
            break;
        }
        if out.iter().any(|(_, data, index)| anicraft_engine::text::face_has(data, *index, c)) {
            continue;
        }
        if let Some(f) = fonts.face_covering(c, prefer, MAX_FACE_BYTES) {
            out.push((f.family, f.data, f.index));
        }
    }
    out
}

/// Once a frame, after everything is painted: takes in a finished search, and starts one for
/// characters that were painted without a glyph.
pub fn tick(app: &mut App, ctx: &egui::Context) {
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !app.font_fallback.enabled {
            return;
        }
        if let Some(rx) = &app.font_fallback.rx {
            match rx.try_recv() {
                Ok(found) => {
                    app.font_fallback.rx = None;
                    if add(&mut app.font_fallback.faces, found) {
                        // The new faces draw from the next frame on.
                        app.apply_prefs(ctx);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => app.font_fallback.rx = None,
            }
            return;
        }
        let fb = &mut app.font_fallback;
        if fb.faces.len() >= MAX_FACES || fb.asked.len() >= MAX_ASKED {
            return;
        }
        let texts = painted(ctx);
        if texts.is_empty() {
            return;
        }
        let room = MAX_PER_SEARCH.min(MAX_ASKED - fb.asked.len());
        let seen = candidates(texts.iter().map(String::as_str), &fb.asked, room);
        if seen.is_empty() {
            return;
        }
        fb.asked.extend(&seen);
        // Missing from the prose font or the data font: names are drawn in both.
        let (body, data) = (crate::theme::font(Type::Body), crate::theme::font(Type::Data));
        let missing: Vec<char> = ctx.fonts_mut(|f| seen.into_iter().filter(|&c| !f.has_glyph(&body, c) || !f.has_glyph(&data, c)).collect());
        if missing.is_empty() {
            return;
        }
        let room = MAX_FACES - fb.faces.len();
        let locale = crate::platform::system_locale();
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        let spawned = std::thread::Builder::new().name("anicraft-font-fallback".into()).spawn(move || {
            let found = find(anicraft_engine::text::Fonts::get(), &missing, &preferred(locale.as_deref()), room);
            let _ = tx.send(found);
            wake.request_repaint();
        });
        if spawned.is_ok() {
            fb.rx = Some(rx);
        }
    }
}

/// Adds the faces not held yet, up to [`MAX_FACES`]; whether any was added.
fn add(faces: &mut Vec<UserFont>, found: Vec<UserFont>) -> bool {
    let before = faces.len();
    for f in found {
        if faces.len() < MAX_FACES && !faces.iter().any(|(name, _, index)| *name == f.0 && *index == f.2) {
            faces.push(f);
        }
    }
    faces.len() > before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_characters_that_need_a_font_are_looked_up_and_each_once() {
        let mut asked = BTreeSet::new();
        let seen = candidates(["Layer 1", "ボール · layer", "a\u{200B}b\u{FE0F}\tc", "ボール"], &asked, 64);
        assert_eq!(seen.iter().collect::<String>(), "·ボルー", "ASCII, spaces and invisible marks are skipped; each character once, in code order");
        asked.extend(&seen);
        assert!(candidates(["ボール"], &asked, 64).is_empty(), "asked once");
        assert_eq!(candidates(["日本語のテキスト"], &asked, 3).len(), 3, "bounded");
        assert!(candidates(["plain", ""], &asked, 64).is_empty());
    }

    #[test]
    fn the_systems_language_orders_the_faces_for_han_characters() {
        let first = |l: Option<&str>| preferred(l).first().copied();
        assert_eq!(first(None), Some("Yu Gothic UI"));
        assert_eq!(first(Some("en-US")), Some("Yu Gothic UI"));
        assert_eq!(first(Some("zh_CN")), Some("Microsoft YaHei UI"));
        assert_eq!(first(Some("KO-kr")), Some("Malgun Gothic"));
        assert_eq!(first(Some("é")), Some("Yu Gothic UI"), "an odd locale is no reason to fail");
        let all = preferred(Some("ja"));
        assert_eq!(all.len(), JAPANESE.len() + CHINESE.len() + KOREAN.len() + OTHERS.len());
        assert!(all.contains(&"Segoe UI") && all.contains(&"Noto Sans Arabic"));
    }

    #[test]
    fn faces_are_kept_once_and_no_more_than_the_limit() {
        let face = |n: &str, i: u32| (n.to_string(), vec![0u8; 4], i);
        let mut faces = Vec::new();
        assert!(add(&mut faces, vec![face("A", 0), face("A", 0), face("A", 1)]));
        assert_eq!(faces.len(), 2, "the same face twice is one; another face of the collection is its own");
        assert!(!add(&mut faces, vec![face("A", 0)]));
        assert!(add(&mut faces, (0..20).map(|i| face("B", i)).collect()));
        assert_eq!(faces.len(), MAX_FACES);
        assert!(!add(&mut faces, vec![face("C", 0)]));
    }

    /// The whole path on this computer's fonts: a character the bundled fonts lack is painted,
    /// the search finds a face that has it, and once installed the interface can draw it.
    /// (Skipped where no installed font has Japanese.)
    #[test]
    fn a_painted_character_without_a_glyph_gets_a_face_that_has_it() {
        let fonts = anicraft_engine::text::Fonts::get();
        let found = find(fonts, &['あ', 'い', '中'], &preferred(None), MAX_FACES);
        if found.is_empty() {
            eprintln!("no installed font has Japanese: skipped");
            return;
        }
        assert!(found.len() <= 2, "one face covers kana and kanji: {:?}", found.iter().map(|f| &f.0).collect::<Vec<_>>());
        assert!(found.iter().all(|(_, data, _)| data.len() <= MAX_FACE_BYTES));
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx, None, None, &[]);
        let body = crate::theme::font(Type::Body);
        let run = |ctx: &egui::Context| {
            let mut texts = Vec::new();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.label(crate::theme::text(Type::Body, "あい layer"));
                texts = painted(ui.ctx());
            });
            out.textures_delta.clear();
            texts
        };
        let texts = run(&ctx);
        assert!(texts.iter().any(|t| t.contains('あ')), "the painted text is read back: {texts:?}");
        assert!(!ctx.fonts_mut(|f| f.has_glyph(&body, 'あ')), "the bundled fonts have no kana");
        crate::theme::install_fonts(&ctx, None, None, &found);
        run(&ctx);
        assert!(ctx.fonts_mut(|f| f.has_glyph(&body, 'あ') && f.has_glyph(&crate::theme::font(Type::Data), 'あ')), "with the fallback, both families draw it");
        assert!(ctx.fonts_mut(|f| f.has_glyph(&body, 'A')));
    }
}
