//! Text commands (ADR 0012): create and edit text blocks, list fonts. Laying text out happens
//! here, when it changes; the document keeps the outlines, so rendering never needs fonts.
//!
//! Dynamic and input text fields (ADR 0066): `text.field` sets a block's Text type and field
//! options. A field's box is fixed and its text is laid out inside the gutter, aligned in the box;
//! [`field_layout`] does that for the commands and for the player while a movie plays.
//!
//! Text on a path (ADR 0070): `text.attach_path` sets static text along a shape's outline (a
//! copy of it, in the text's own space), `text.path` moves it along, flips it or releases it.
//! The glyphs are set by arc length and stored as outlines, as all text is.
//!
//! Styled text (ADR 0085): a block's characters come in runs with their own font, size, style,
//! colour, letter spacing, baseline shift, underline and link, and its paragraphs have their own
//! alignment, indent, margins and line spacing. `text.style` styles a range of characters,
//! `text.paragraph` the paragraphs a range touches, `text.styles` reads them back (mixed values
//! as `null`); `text.edit` keeps the runs through a change of text (typing continues the style
//! before it) or takes new ones. Every layout here goes through [`lay_out_block`].

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::rich_text::{self as rt, StyleSummary};
use anicraft_doc::{BaselineShift, CharStyle, ColorEffect, Element, ElementId, ElementKind, FIELD_GUTTER, LineType, MAX_FIELD_CHARS, MAX_FIELD_NAME, MAX_PATH_OFFSET, MAX_TEXT_RUNS, MAX_URL, ParagraphStyle, Rgba, TextAlign, TextBlock, TextField, TextKind, TextPath, TextRun};
use anicraft_geom::{Affine, Path, Vec2};
use anicraft_text::{Align, Fonts};
use serde_json::{Value, json};
use std::sync::Arc;

/// The text style new text gets: the last one used, as in Animate.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub font: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: Rgba,
    pub align: TextAlign,
    pub letter_spacing: f32,
    pub line_spacing: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle { font: String::new(), size: 32.0, bold: false, italic: false, color: Rgba::BLACK, align: TextAlign::Left, letter_spacing: 0.0, line_spacing: 0.0 }
    }
}

const CHAR_KEYS: &str = "font?, size? (px), bold?, italic?, color?, letter_spacing?, shift?: normal|superscript|subscript, underline?, url? (\"\": none)";
const PARA_KEYS: &str = "align?: left|center|right, indent? (px, first line), left_margin?, right_margin? (px), line_spacing? (px)";

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, mutates, run| CommandSpec { id, label, menu, shortcut, params, mutates, run };
    // The four params docs put together from the shared key lists: made once and kept, since
    // the registry is rebuilt by every caller (the menu bar asks each frame; #454: a fresh
    // copy each time was never freed, about 1.5 kB a frame).
    static DOCS: std::sync::OnceLock<[String; 4]> = std::sync::OnceLock::new();
    let [create_doc, edit_doc, style_doc, paragraph_doc] = DOCS.get_or_init(|| {
        [
            format!("{{text (may be empty for an input field), type?: static|dynamic|input, x?, y? (top-left; default: centred), width?, {CHAR_KEYS}, {PARA_KEYS}, runs?: [{{chars, …character keys}}] (styles of the text's characters in order; unset keys take the style above), paragraphs?: [{{…paragraph keys}}] (one per line of the text)}} (ADR 0085)"),
            format!("{{text? (the runs follow the change: typed characters continue the style before them), runs?, paragraphs? (as text.create: replace the styles), width? (px, wraps; null: one line per paragraph), {CHAR_KEYS} (every character), {PARA_KEYS} (every paragraph)}} on the selected text"),
            format!("{{start?, end? (characters; default: all), {CHAR_KEYS}}} styles those characters of the selected text, splitting and joining its runs (ADR 0085)"),
            format!("{{start?, end? (characters; default: all), {PARA_KEYS}}} sets the layout of the paragraphs those characters touch in the selected text (ADR 0085)"),
        ]
    });
    vec![
        c("text.create", "New Text", "", None, create_doc.as_str(), true, create as crate::RunFn),
        c("text.edit", "Edit Text", "", None, edit_doc.as_str(), true, |s, p| s.each_layer(false, |s| edit(s, p))),
        c("text.style", "Text Character Style", "", None, style_doc.as_str(), true, |s, p| s.each_layer(false, |s| style_range(s, p))),
        c("text.paragraph", "Text Paragraph Style", "", None, paragraph_doc.as_str(), true, |s, p| s.each_layer(false, |s| paragraph_range(s, p))),
        c("text.styles", "Text Styles", "", None, "{start?, end? (characters; default: all)} of the first selected text: its runs, paragraphs, and what those characters share (a mixed value is null) (ADR 0085)", false, styles_of),
        c("text.fonts", "Fonts", "", None, "{}", false, fonts),
        c(
            "text.attach_path",
            "Attach Text to Path",
            "Modify/Text",
            None,
            "{path?: element id of a shape (default: the other selected element, a shape), x?, y? (pick the shape at this stage point instead), offset? (px along the path; default by the text's alignment: its start, middle or end), flip?: bool} sets the selected static text along a copy of the shape's outline (its first contour); the shape stays as it is (ADR 0070)",
            true,
            attach_path as crate::RunFn,
        ),
        c("text.path", "Text on Path", "", None, "{offset? (px along the path), flip?: bool (along the path backwards, on its other side), release?: bool (back to lines of text where it starts)} on the selected text on a path (ADR 0070)", true, |s, p| s.each_layer(false, |s| set_path(s, p))),
        c("text.release_path", "Release Text from Path", "Modify/Text", None, "{} the selected text on a path goes back to lines of text (ADR 0070)", true, |s, _| s.each_layer(false, |s| set_path(s, &json!({"release": true})))),
        c("text.field", "Text Type", "", None, "{type?: static|dynamic|input, lines?: single|multiline|multiline_no_wrap|password (input only), width?, height? (the field's box, px), selectable?, border?, background?, border_color?, background_color?, max_chars? (input; 0: no limit), variable? (kept, not run), embed?: characters whose glyph outlines the HTML export carries for this field's font, so text a script sets is drawn in it where the font's licence allows (\"\": none; ADR 0107)} on the selected text; dynamic and input fields are named with instance.set_name and scripted in Test Movie and the HTML export (ADR 0066)", true, |s, p| s.each_layer(false, |s| set_field(s, p))),
    ]
}

fn align_of(p: &Value, key: &str) -> Result<Option<TextAlign>> {
    Ok(match opt_str(p, key)? {
        None => None,
        Some("left") => Some(TextAlign::Left),
        Some("center" | "centre") => Some(TextAlign::Center),
        Some("right") => Some(TextAlign::Right),
        Some(other) => return Err(EngineError::Param(format!("'{key}' is left, center or right, not '{other}'"))),
    })
}

/// `width`: a positive number wraps, `null` doesn't, absent keeps `current`.
fn width_param(p: &Value, current: Option<f32>) -> Result<Option<f32>> {
    match p.get("width") {
        None => Ok(current),
        Some(Value::Null) => Ok(None),
        Some(_) => match opt_f32(p, "width")? {
            Some(w) if w > 0.0 => Ok(Some(w.min(100_000.0))),
            _ => Err(EngineError::Param("'width' must be a positive number of pixels, or null".into())),
        },
    }
}

fn ranged(p: &Value, key: &str, lo: f32, hi: f32) -> Result<Option<f32>> {
    match opt_f32(p, key)? {
        Some(v) if !(lo..=hi).contains(&v) => Err(EngineError::Param(format!("'{key}' must be between {lo} and {hi} px"))),
        v => Ok(v),
    }
}

/// Character style keys in a command's params: each one given changes that attribute only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharPatch {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<Rgba>,
    pub letter_spacing: Option<f32>,
    pub shift: Option<BaselineShift>,
    pub underline: Option<bool>,
    pub url: Option<String>,
}

impl CharPatch {
    pub fn parse(p: &Value) -> Result<CharPatch> {
        let shift = opt_str(p, "shift")?.map(|k| BaselineShift::from_key(k).ok_or_else(|| EngineError::Param(format!("'shift' is normal, superscript or subscript, not '{k}'")))).transpose()?;
        let url = match opt_str(p, "url")? {
            None => None,
            Some(u) if u.chars().count() > MAX_URL => return Err(EngineError::Param(format!("'url' is at most {MAX_URL} characters"))),
            Some(u) => Some(rt::safe_url(u).ok_or_else(|| EngineError::Param("'url' can't run script (javascript:, vbscript: and data: links aren't allowed)".into()))?),
        };
        Ok(CharPatch {
            font: opt_str(p, "font")?.map(|f| f.chars().take(256).collect()),
            size: ranged(p, "size", 1.0, 2000.0)?,
            bold: opt_bool(p, "bold")?,
            italic: opt_bool(p, "italic")?,
            color: opt_color(p, "color")?,
            letter_spacing: ranged(p, "letter_spacing", -1000.0, 1000.0)?,
            shift,
            underline: opt_bool(p, "underline")?,
            url,
        })
    }
    pub fn is_empty(&self) -> bool {
        *self == CharPatch::default()
    }
    pub fn apply(&self, s: &mut CharStyle) {
        if let Some(v) = &self.font {
            s.font.clone_from(v);
        }
        if let Some(v) = self.size {
            s.size = v;
        }
        if let Some(v) = self.bold {
            s.bold = v;
        }
        if let Some(v) = self.italic {
            s.italic = v;
        }
        if let Some(v) = self.color {
            s.color = v;
        }
        if let Some(v) = self.letter_spacing {
            s.letter_spacing = v;
        }
        if let Some(v) = self.shift {
            s.shift = v;
        }
        if let Some(v) = self.underline {
            s.underline = v;
        }
        if let Some(v) = &self.url {
            s.url.clone_from(v);
        }
    }
}

/// Paragraph style keys in a command's params.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParaPatch {
    pub align: Option<TextAlign>,
    pub indent: Option<f32>,
    pub left_margin: Option<f32>,
    pub right_margin: Option<f32>,
    pub line_spacing: Option<f32>,
}

impl ParaPatch {
    pub fn parse(p: &Value) -> Result<ParaPatch> {
        Ok(ParaPatch {
            align: align_of(p, "align")?,
            indent: ranged(p, "indent", -1000.0, 1000.0)?,
            left_margin: ranged(p, "left_margin", 0.0, 1000.0)?,
            right_margin: ranged(p, "right_margin", 0.0, 1000.0)?,
            line_spacing: ranged(p, "line_spacing", -1000.0, 1000.0)?,
        })
    }
    pub fn is_empty(&self) -> bool {
        *self == ParaPatch::default()
    }
    pub fn apply(&self, s: &mut ParagraphStyle) {
        s.align = self.align.unwrap_or(s.align);
        s.indent = self.indent.unwrap_or(s.indent);
        s.left_margin = self.left_margin.unwrap_or(s.left_margin);
        s.right_margin = self.right_margin.unwrap_or(s.right_margin);
        s.line_spacing = self.line_spacing.unwrap_or(s.line_spacing);
    }
}

/// `runs: [{chars, …character keys}]` over `base` (unset keys take it), at most
/// [`MAX_TEXT_RUNS`] of them.
fn runs_param(p: &Value, base: &CharStyle) -> Result<Option<Vec<TextRun>>> {
    let Some(v) = p.get("runs").filter(|v| !v.is_null()) else { return Ok(None) };
    let list = v.as_array().ok_or_else(|| EngineError::Param("'runs' must be a list of {chars, …style}".into()))?;
    if list.len() > MAX_TEXT_RUNS {
        return Err(EngineError::Param(format!("'runs' has at most {MAX_TEXT_RUNS} runs")));
    }
    let mut out = Vec::with_capacity(list.len());
    for r in list {
        let chars = opt_u32(r, "chars")?.ok_or_else(|| EngineError::Param("each run needs 'chars' (how many characters it styles)".into()))?;
        let mut style = base.clone();
        CharPatch::parse(r)?.apply(&mut style);
        out.push(TextRun { chars, style });
    }
    Ok(Some(out))
}

/// `paragraphs: [{…paragraph keys}]` over `base`.
fn paragraphs_param(p: &Value, base: &ParagraphStyle) -> Result<Option<Vec<ParagraphStyle>>> {
    let Some(v) = p.get("paragraphs").filter(|v| !v.is_null()) else { return Ok(None) };
    let list = v.as_array().ok_or_else(|| EngineError::Param("'paragraphs' must be a list of {align?, indent?, …}".into()))?;
    if list.len() > anicraft_text::MAX_CHARS + 1 {
        return Err(EngineError::Param("'paragraphs' has more entries than a block has lines".into()));
    }
    list.iter()
        .map(|q| {
            let mut s = *base;
            ParaPatch::parse(q)?.apply(&mut s);
            Ok(s)
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// `start`, `end` (characters, default the whole text), clamped to `n`.
fn range_param(p: &Value, n: u32) -> Result<(u32, u32)> {
    let start = opt_u32(p, "start")?.unwrap_or(0).min(n);
    let end = opt_u32(p, "end")?.unwrap_or(n).min(n);
    if end < start {
        return Err(EngineError::Param("'end' comes before 'start'".into()));
    }
    Ok((start, end))
}

fn align_text(a: TextAlign) -> Align {
    match a {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    }
}

/// `d`'s runs and paragraphs as the text engine takes them, and the paints (colour, link) the
/// runs' `paint` numbers stand for.
fn rich_parts(d: &TextBlock) -> (Vec<anicraft_text::Run>, Vec<anicraft_text::ParaStyle>, Vec<(Rgba, String)>) {
    let mut paints: Vec<(Rgba, String)> = Vec::new();
    let runs = d
        .styled_runs()
        .into_iter()
        .map(|r| {
            let key = (r.style.color, r.style.url.clone());
            let paint = match paints.iter().position(|k| *k == key) {
                Some(i) => i,
                None => {
                    paints.push(key);
                    paints.len() - 1
                }
            } as u32;
            let shift = match r.style.shift {
                BaselineShift::Normal => anicraft_text::Shift::Normal,
                BaselineShift::Superscript => anicraft_text::Shift::Superscript,
                BaselineShift::Subscript => anicraft_text::Shift::Subscript,
            };
            anicraft_text::Run {
                chars: r.chars as usize,
                style: anicraft_text::RunStyle { family: r.style.font, size: r.style.size, bold: r.style.bold, italic: r.style.italic, letter_spacing: r.style.letter_spacing, shift, underline: r.style.underline, paint },
            }
        })
        .collect();
    let paras = d
        .paragraph_styles()
        .into_iter()
        .map(|p| anicraft_text::ParaStyle { align: align_text(p.align), indent: p.indent, left_margin: p.left_margin, right_margin: p.right_margin, line_spacing: p.line_spacing })
        .collect();
    (runs, paras, paints)
}

/// The block with its outline from laid-out pieces and its runs' fonts the families actually
/// used; with a note when a family asked for isn't installed.
fn finish(mut d: TextBlock, pieces: Vec<(u32, Path)>, used: Used, paints: &[(Rgba, String)]) -> (TextBlock, Option<String>) {
    let mut notes: Vec<String> = Vec::new();
    let mut runs = d.styled_runs();
    for (r, used) in runs.iter_mut().zip(used.families) {
        let asked = r.style.font.trim();
        if notes.is_empty() && !asked.is_empty() && !used.eq_ignore_ascii_case(asked) {
            notes.push(format!("“{asked}” isn't installed on this computer; used {used}"));
        }
        r.style.font.clone_from(used);
    }
    notes.extend(borrowed_note(used.fallbacks, used.missing));
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    let paras = d.paragraph_styles();
    d.set_styles(runs, paras);
    let pieces = pieces.into_iter().map(|(k, p)| paints.get(k as usize).map_or((d.color, String::new(), p.clone()), |(c, u)| (*c, u.clone(), p))).collect();
    d.set_outline(pieces);
    (d, note)
}

/// What a layout drew a block with: each run's family, the families that lent letters the
/// runs' own faces lack, and how many characters no face has (ADR 0130).
struct Used<'a> {
    families: &'a [String],
    fallbacks: &'a [String],
    missing: usize,
}

/// What to tell the user when a block's fonts don't have all of its letters: which families
/// the others were drawn from, and how many show as boxes because no installed font has them.
/// The block keeps the font that was chosen, so nothing else says so.
pub(crate) fn borrowed_note(fallbacks: &[String], missing: usize) -> Option<String> {
    let mut notes = Vec::new();
    match fallbacks {
        [] => {}
        [one] => notes.push(format!("letters the font doesn't have are drawn in {one}")),
        [first, rest @ ..] => {
            let shown: Vec<&str> = rest.iter().take(3).map(String::as_str).collect();
            let more = if rest.len() > shown.len() { " and other fonts" } else { "" };
            notes.push(format!("letters the font doesn't have are drawn in {first}, {}{more}", shown.join(", ")));
        }
    }
    match missing {
        0 => {}
        1 => notes.push("1 character isn't in any installed font and shows as a box".to_string()),
        n => notes.push(format!("{n} characters aren't in any installed font and show as boxes")),
    }
    (!notes.is_empty()).then(|| notes.join("; "))
}

fn layout_err(e: anicraft_text::TextError) -> EngineError {
    EngineError::State(format!("can't lay out text: {e}"))
}

/// Lays out a block from its text, runs, paragraphs and width, as static text, a field (in its
/// box) or text on a path, with the fonts on this computer. Returns the laid-out block, a note
/// when a family wasn't installed, and (text on a path) where the block's origin moved to,
/// which the element's transform must move by for nothing to move (`transform ×
/// translate(shift)`; zero otherwise).
pub(crate) fn lay_out_block(d: &TextBlock) -> Result<(TextBlock, Option<String>, Vec2)> {
    let fonts = Fonts::get();
    let mut d = d.clone();
    let (runs, paras, paints) = rich_parts(&d);
    match (d.field.clone(), d.on_path.clone()) {
        (_, Some(on)) => {
            // Lines along the path (ADR 0107): laid out left-aligned with each paragraph's
            // line spacing; the block's alignment anchors every line along the path.
            let lines: Vec<anicraft_text::ParaStyle> = paras.iter().map(|q| anicraft_text::ParaStyle { line_spacing: q.line_spacing, ..anicraft_text::ParaStyle::default() }).collect();
            let l = fonts.layout_rich(&anicraft_text::RichText { text: &d.text, runs: &runs, paragraphs: &lines, width: None }, true).map_err(layout_err)?;
            let offset = if on.offset.is_finite() { on.offset.clamp(-MAX_PATH_OFFSET, MAX_PATH_OFFSET) } else { 0.0 };
            let set = anicraft_text::on_path_boxed(&l.glyphs, &on.path, anicraft_text::PathPlacement { offset, flip: on.flip, align: align_text(d.align) }).ok_or_else(|| EngineError::State("the path has no length to set text along".into()))?;
            d.width = None;
            d.field = None;
            d.box_width = set.width;
            d.box_height = set.height;
            d.on_path = Some(TextPath { path: set.path, offset, flip: on.flip });
            let shift = set.shift;
            let (b, n) = finish(d, set.pieces, Used { families: &l.families, fallbacks: &l.fallbacks, missing: l.missing }, &paints);
            Ok((b, n, shift))
        }
        (Some(f), None) => {
            d.text = d.text.chars().take(MAX_FIELD_CHARS).collect();
            let (runs, paras, paints) = rich_parts(&d);
            let shown = f.shown(&d.text);
            let l = field_lay(fonts, &anicraft_text::RichText { text: &shown, runs: &runs, paragraphs: &paras, width: f.wrap_width(d.box_width) }, d.align, &f, d.box_width).map_err(layout_err)?;
            d.width = f.wrap_width(d.box_width);
            let used = Used { families: &l.families, fallbacks: &l.fallbacks, missing: l.missing };
            let (b, n) = finish(d, l.pieces, used, &paints);
            Ok((b, n, Vec2::ZERO))
        }
        (None, None) => {
            let l = fonts.layout_rich(&anicraft_text::RichText { text: &d.text, runs: &runs, paragraphs: &paras, width: d.width }, false).map_err(layout_err)?;
            d.box_width = l.width;
            d.box_height = l.height;
            let used = Used { families: &l.families, fallbacks: &l.fallbacks, missing: l.missing };
            let (b, n) = finish(d, l.pieces, used, &paints);
            Ok((b, n, Vec2::ZERO))
        }
    }
}

/// A field's rich text laid out in its box (ADR 0066): wrapped at the box less its gutters
/// when multiline, the widest line aligned in the box when it doesn't wrap, and the outline
/// (and its pieces) moved into the gutter. The carets are from the text's left edge.
fn field_lay(fonts: &Fonts, rich: &anicraft_text::RichText, align: TextAlign, field: &TextField, box_width: f32) -> std::result::Result<anicraft_text::Layout, anicraft_text::TextError> {
    let mut l = fonts.layout_rich(rich, false)?;
    let inner = (box_width - 2.0 * FIELD_GUTTER).max(0.0);
    let shift = match (field.wrap_width(box_width), align) {
        (Some(_), _) | (None, TextAlign::Left) => 0.0,
        (None, TextAlign::Center) => ((inner - l.width) * 0.5).max(0.0),
        (None, TextAlign::Right) => (inner - l.width).max(0.0),
    };
    let to = Affine::translate(FIELD_GUTTER + shift, FIELD_GUTTER);
    l.outline = l.outline.transformed(&to);
    for (_, p) in &mut l.pieces {
        *p = p.transformed(&to);
    }
    if shift != 0.0 {
        for line in &mut l.carets {
            for s in &mut line.stops {
                s.1 += shift;
            }
            for b in &mut line.boxes {
                b.x0 += shift;
                b.x1 += shift;
            }
        }
        l.width += shift;
    }
    Ok(l)
}

/// A field's text laid out in one style, as the player lays out text a script set or a viewer
/// typed (ADR 0066).
pub fn field_layout(fonts: &Fonts, style: &TextStyle, field: &TextField, box_width: f32, shown: &str) -> std::result::Result<anicraft_text::Layout, anicraft_text::TextError> {
    let run = anicraft_text::Run { chars: usize::MAX, style: anicraft_text::RunStyle { family: style.font.clone(), size: style.size, bold: style.bold, italic: style.italic, letter_spacing: style.letter_spacing, ..anicraft_text::RunStyle::default() } };
    let para = anicraft_text::ParaStyle { align: align_text(style.align), line_spacing: style.line_spacing, ..anicraft_text::ParaStyle::default() };
    field_lay(fonts, &anicraft_text::RichText { text: shown, runs: &[run], paragraphs: &[para], width: field.wrap_width(box_width) }, style.align, field, box_width)
}

/// A field's text laid out in the styles a script gave it with `htmlText` (ADR 0107), and the
/// colour each of the layout's paints stands for. Links don't cut pieces here: a colour is one
/// piece.
pub fn field_layout_rich(fonts: &Fonts, block: &TextBlock, field: &TextField, shown: &str, rich: &anicraft_doc::html_text::RichStyles) -> std::result::Result<(anicraft_text::Layout, Vec<Rgba>), anicraft_text::TextError> {
    let mut d = TextBlock { text: shown.to_string(), runs: Vec::new(), paragraphs: Vec::new(), ..block.clone() };
    let runs = rich.runs.iter().map(|r| TextRun { chars: r.chars, style: CharStyle { url: String::new(), ..r.style.clone() } }).collect();
    d.set_styles(runs, rich.paragraphs.clone());
    let (runs, paras, paints) = rich_parts(&d);
    let l = field_lay(fonts, &anicraft_text::RichText { text: shown, runs: &runs, paragraphs: &paras, width: field.wrap_width(block.box_width) }, d.align, field, block.box_width)?;
    Ok((l, paints.into_iter().map(|(c, _)| c).collect()))
}

/// `t` with its text changed to `text`, its runs and paragraphs following the change (one
/// edit: what was typed continues the style before it).
pub(crate) fn retext(t: &TextBlock, text: &str) -> TextBlock {
    let mut d = t.clone();
    if d.text == text {
        return d;
    }
    let (start, old_end, new_end) = rt::diff_chars(&t.text, text);
    let runs = rt::type_into_runs(&t.styled_runs(), start, old_end, new_end - start);
    let inserted: String = text.chars().skip(start as usize).take((new_end - start) as usize).collect();
    let paras = rt::edit_paragraphs(&t.paragraph_styles(), &t.text, start, old_end, &inserted);
    d.text = text.to_string();
    d.set_styles(runs, paras);
    d
}

/// What Find and Replace lays text out with: the runs following the change, as static text
/// (its fields' boxes dropped), except that text on a path stays on its path.
pub(crate) fn relay_static(t: &TextBlock, text: &str) -> Result<(TextBlock, Option<String>, Vec2)> {
    let mut d = retext(t, text);
    d.field = None;
    lay_out_block(&d)
}

/// `t` with every run's font set to `family`, laid out as [`relay_static`] does.
pub(crate) fn refont_static(t: &TextBlock, family: &str) -> Result<(TextBlock, Option<String>, Vec2)> {
    let mut d = t.clone();
    let runs = rt::restyle_runs(&t.styled_runs(), 0, t.char_count(), |s| s.font = family.to_string());
    let paras = d.paragraph_styles();
    d.set_styles(runs, paras);
    d.field = None;
    lay_out_block(&d)
}

/// The selected static text, along a copy of a shape's outline (ADR 0070).
fn attach_path(s: &mut Session, p: &Value) -> Result<Value> {
    let by_id = opt_u64(p, "path")?.map(ElementId);
    let at = match (opt_f32(p, "x")?, opt_f32(p, "y")?) {
        (Some(x), Some(y)) => Some(Vec2::new(x, y)),
        (None, None) => None,
        _ => return Err(EngineError::Param("give both 'x' and 'y', or neither".into())),
    };
    let flip = opt_bool(p, "flip")?.unwrap_or(false);
    let offset = opt_f32(p, "offset")?;
    if offset.is_some_and(|o| !(-MAX_PATH_OFFSET..=MAX_PATH_OFFSET).contains(&o)) {
        return Err(EngineError::Param(format!("'offset' must be within ±{MAX_PATH_OFFSET} px")));
    }
    // The shape: by id or by point, on any layer of what is being edited; else the selection's.
    let wanted = match (by_id, at) {
        (Some(id), _) => Some(id),
        (None, Some(pt)) => Some(s.hit_test(pt, 2.0).map(|(_, e)| e).ok_or_else(|| EngineError::State("there is no shape there to set the text along".into()))?),
        (None, None) => None,
    };
    let texts: Vec<ElementId> = s.selected_elements_mut()?.iter().filter(|e| matches!(&e.kind, ElementKind::Text(t) if t.field.is_none())).map(|e| e.id).collect();
    let [text_id] = texts.as_slice() else {
        return Err(EngineError::State(if texts.is_empty() { "select a static text block to set along a path".into() } else { "select one text block to set along a path".into() }));
    };
    // The shapes on the timeline being edited at this frame (in its space, as the text is):
    // the one asked for, or the selected one.
    let selected = s.selection.on(s.layer);
    let mut candidates: Vec<Path> = Vec::new();
    for l in &s.timeline().layers {
        let Some((_, els)) = anicraft_doc::evaluate_layer_elements(l, s.frame) else { continue };
        for e in els.iter() {
            let ElementKind::Shape(sh) = &e.kind else { continue };
            let pick = match wanted {
                Some(w) => w == e.id,
                None => l.id == s.layer && selected.contains(&e.id),
            };
            if pick {
                candidates.push(sh.path.transformed(&e.transform));
            }
        }
    }
    let path = match candidates.as_slice() {
        [path] => path.clone(),
        [] => return Err(EngineError::State("pick a shape to set the text along".into())),
        _ => return Err(EngineError::State("select the text and one shape (or pick the shape)".into())),
    };
    // Its first contour, in the timeline's space (the text takes the path's place).
    let first: Path = Path { cmds: path.cmds.iter().enumerate().take_while(|(i, c)| *i == 0 || !matches!(c, anicraft_geom::PathCmd::MoveTo(_))).map(|(_, c)| *c).collect() };
    let length = anicraft_geom::walk::CurveWalk::new(&first).map(|w| w.length()).ok_or_else(|| EngineError::State("the shape's outline has no length to set text along".into()))?;
    let mut out = Value::Null;
    for e in s.selected_elements_mut()? {
        if e.id != *text_id {
            continue;
        }
        let ElementKind::Text(t) = &e.kind else { continue };
        let offset = offset.unwrap_or(match t.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => length * 0.5,
            TextAlign::Right => length,
        });
        let (block, note, shift) = lay_out_block(&TextBlock { field: None, width: None, on_path: Some(TextPath { path: first.clone(), offset, flip }), ..(**t).clone() })?;
        // The path is in stage space; the text's new space is the path's, moved to its box.
        e.transform = Affine::translate(shift.x, shift.y);
        out = json!({"element": e.id.0, "length": length, "offset": offset, "flip": flip, "note": note});
        e.kind = ElementKind::Text(Arc::new(block));
    }
    s.select_only([*text_id]);
    Ok(out)
}

/// Moves, flips or releases the selected text on a path.
fn set_path(s: &mut Session, p: &Value) -> Result<Value> {
    let offset = opt_f32(p, "offset")?;
    if offset.is_some_and(|o| !(-MAX_PATH_OFFSET..=MAX_PATH_OFFSET).contains(&o)) {
        return Err(EngineError::Param(format!("'offset' must be within ±{MAX_PATH_OFFSET} px")));
    }
    let flip = opt_bool(p, "flip")?;
    let release = opt_bool(p, "release")?.unwrap_or(false);
    let mut changed = 0;
    for e in s.selected_elements_mut()? {
        let ElementKind::Text(t) = &e.kind else { continue };
        let Some(on) = &t.on_path else { continue };
        if release {
            // Back to a line of text, its box where the text started along the path.
            let start = t.outline.bounds();
            let (block, _, _) = lay_out_block(&TextBlock { on_path: None, width: None, ..(**t).clone() })?;
            let at = if start.min.x.is_finite() { start.min } else { Vec2::ZERO };
            e.transform = e.transform.mul(&Affine::translate(at.x, at.y));
            e.kind = ElementKind::Text(Arc::new(block));
        } else {
            let on = TextPath { offset: offset.unwrap_or(on.offset), flip: flip.unwrap_or(on.flip), ..on.clone() };
            let (block, _, shift) = lay_out_block(&TextBlock { on_path: Some(on), ..(**t).clone() })?;
            e.transform = e.transform.mul(&Affine::translate(shift.x, shift.y));
            e.kind = ElementKind::Text(Arc::new(block));
        }
        changed += 1;
    }
    if changed == 0 {
        return Err(EngineError::State("the selection has no text on a path".into()));
    }
    Ok(json!({"changed": changed}))
}

/// The box a block becomes a field in: the text's own size and the gutters (at least 20 px wide).
fn field_box(t: &TextBlock) -> (f32, f32) {
    ((t.width.unwrap_or(t.box_width) + 2.0 * FIELD_GUTTER).max(20.0), t.box_height + 2.0 * FIELD_GUTTER)
}

fn kind_param(p: &Value) -> Result<Option<TextKind>> {
    opt_str(p, "type")?.map(|k| TextKind::from_key(k).ok_or_else(|| EngineError::Param(format!("'type' is static, dynamic or input, not '{k}'")))).transpose()
}

fn set_field(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = kind_param(p)?;
    let lines = opt_str(p, "lines")?.map(|k| LineType::from_key(k).ok_or_else(|| EngineError::Param(format!("'lines' is single, multiline, multiline_no_wrap or password, not '{k}'")))).transpose()?;
    let size = |key: &str| -> Result<Option<f32>> {
        match opt_f32(p, key)? {
            Some(v) if (1.0..=100_000.0).contains(&v) => Ok(Some(v)),
            Some(_) => Err(EngineError::Param(format!("'{key}' must be between 1 and 100000 px"))),
            None => Ok(None),
        }
    };
    let (width, height) = (size("width")?, size("height")?);
    let max_chars = opt_u32(p, "max_chars")?;
    if max_chars.is_some_and(|n| n as usize > MAX_FIELD_CHARS) {
        return Err(EngineError::Param(format!("'max_chars' is at most {MAX_FIELD_CHARS}")));
    }
    let variable = opt_str(p, "variable")?.map(|v| v.trim().chars().take(MAX_FIELD_NAME).collect::<String>());
    let embed = match opt_str(p, "embed")? {
        Some(e) if e.chars().count() > anicraft_doc::MAX_EMBED_CHARS * 4 => return Err(EngineError::Param(format!("'embed' is at most {} characters", anicraft_doc::MAX_EMBED_CHARS * 4))),
        Some(e) => Some(anicraft_doc::embed_chars(e)),
        None => None,
    };
    let (selectable, border, background) = (opt_bool(p, "selectable")?, opt_bool(p, "border")?, opt_bool(p, "background")?);
    let (border_color, background_color) = (opt_color(p, "border_color")?, opt_color(p, "background_color")?);
    let mut changed = 0;
    let mut notes = Vec::new();
    for e in s.selected_elements_mut()? {
        let ElementKind::Text(t) = &e.kind else { continue };
        if t.on_path.is_some() && kind.is_some_and(|k| k != TextKind::Static) {
            return Err(EngineError::State("text on a path is static text (release it from the path first)".into()));
        }
        let to = kind.unwrap_or(t.kind());
        let (block, note) = if to == TextKind::Static {
            if t.field.is_none() {
                continue;
            }
            // Back to static text: laid out as before fields, the words where they were.
            e.transform = e.transform.mul(&Affine::translate(FIELD_GUTTER, FIELD_GUTTER));
            e.name.clear();
            let (b, n, _) = lay_out_block(&TextBlock { field: None, width: t.field.as_ref().and_then(|f| f.wrap_width(t.box_width)), ..(**t).clone() })?;
            (b, n)
        } else {
            let (bw, bh) = if t.field.is_some() { (t.box_width, t.box_height) } else { field_box(t) };
            // Wrapped text becomes a multiline field, text with line breaks a multiline one
            // that doesn't wrap, the rest a single line.
            let lines_of = if t.width.is_some() {
                LineType::Multiline
            } else if t.text.contains('\n') {
                LineType::MultilineNoWrap
            } else {
                LineType::Single
            };
            let mut f = t.field.clone().unwrap_or_else(|| TextField { lines: lines_of, ..TextField::new(to) });
            if t.field.is_none() {
                // The text stays where it was; the box grows round it.
                e.transform = e.transform.mul(&Affine::translate(-FIELD_GUTTER, -FIELD_GUTTER));
            }
            f.kind = to;
            if let Some(l) = lines {
                if l == LineType::Password && to != TextKind::Input {
                    return Err(EngineError::Param("only input text can be a password field".into()));
                }
                f.lines = l;
            }
            if f.lines == LineType::Password && to != TextKind::Input {
                f.lines = LineType::Single;
            }
            f.selectable = selectable.unwrap_or(f.selectable);
            f.border = border.unwrap_or(f.border);
            f.background = background.unwrap_or(f.background);
            f.border_color = border_color.unwrap_or(f.border_color);
            f.background_color = background_color.unwrap_or(f.background_color);
            f.max_chars = max_chars.unwrap_or(f.max_chars);
            if let Some(v) = &variable {
                f.variable.clone_from(v);
            }
            if let Some(e) = &embed {
                f.embed.clone_from(e);
            }
            let (b, n, _) = lay_out_block(&TextBlock { field: Some(f.sanitized()), box_width: width.unwrap_or(bw), box_height: height.unwrap_or(bh), ..(**t).clone() })?;
            (b, n)
        };
        notes.extend(note);
        e.kind = ElementKind::Text(Arc::new(block));
        changed += 1;
    }
    if changed == 0 {
        return Err(EngineError::State("the selection has no text to change".into()));
    }
    notes.dedup();
    Ok(json!({"changed": changed, "note": notes.first()}))
}


/// The style a block starts with: `base`, with the params' character and paragraph keys.
fn new_style(p: &Value, base: &TextStyle) -> Result<(CharStyle, ParagraphStyle)> {
    let mut c = CharStyle { font: base.font.clone(), size: base.size, bold: base.bold, italic: base.italic, color: base.color, letter_spacing: base.letter_spacing, ..CharStyle::default() };
    CharPatch::parse(p)?.apply(&mut c);
    let mut q = ParagraphStyle { align: base.align, line_spacing: base.line_spacing, ..ParagraphStyle::default() };
    ParaPatch::parse(p)?.apply(&mut q);
    Ok((c, q))
}

/// The last style used, from a block (what new text gets next).
fn remember(s: &mut Session, b: &TextBlock) {
    s.text_style = TextStyle { font: b.font.clone(), size: b.size, bold: b.bold, italic: b.italic, color: b.color, align: b.align, letter_spacing: b.letter_spacing, line_spacing: b.line_spacing };
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let text: String = str_req(p, "text")?.to_string();
    let kind = kind_param(p)?.unwrap_or_default();
    // An input field may start empty (the viewer fills it in); static text may not.
    if text.trim().is_empty() && kind == TextKind::Static {
        return Err(EngineError::Param("'text' is empty".into()));
    }
    if text.chars().count() > anicraft_text::MAX_CHARS {
        return Err(EngineError::Param(format!("text is limited to {} characters per block", anicraft_text::MAX_CHARS)));
    }
    let (cs, ps) = new_style(p, &s.text_style)?;
    let width = width_param(p, None)?;
    let mut d = TextBlock { text: text.clone(), width, ..TextBlock::default() };
    let runs = runs_param(p, &cs)?.unwrap_or_else(|| vec![TextRun { chars: u32::MAX, style: cs.clone() }]);
    let paras = paragraphs_param(p, &ps)?.unwrap_or_else(|| vec![ps]);
    d.set_styles(runs, paras);
    let (block, note, _) = lay_out_block(&d)?;
    let (block, note) = if kind == TextKind::Static {
        (block, note)
    } else {
        let (bw, bh) = field_box(&block);
        let lines = if width.is_some() {
            LineType::Multiline
        } else if text.contains('\n') {
            LineType::MultilineNoWrap
        } else {
            LineType::Single
        };
        let f = TextField { lines, ..TextField::new(kind) };
        let (b, n, _) = lay_out_block(&TextBlock { field: Some(f), box_width: bw, box_height: bh, ..block })?;
        (b, n)
    };
    let x = opt_f32(p, "x")?.unwrap_or((s.doc.stage.width as f32 - block.box_width) * 0.5);
    let y = opt_f32(p, "y")?.unwrap_or((s.doc.stage.height as f32 - block.box_height) * 0.5);
    let id = ElementId(s.doc.alloc_id());
    let out = json!({"element": id.0, "font": block.font, "width": block.box_width, "height": block.box_height, "runs": block.runs.len().max(1), "note": note});
    remember(s, &block);
    let el = Element { id, name: String::new(), transform: Affine::translate(x, y), color: ColorEffect::default(), transform_3d: None, kind: ElementKind::Text(Arc::new(block)) };
    s.editable_keyframe()?.elements.push(Arc::new(el));
    s.select_only([id]);
    Ok(out)
}

/// Lays `d` out where `e` stands (text on a path keeps its place) and puts it in.
fn put(e: &mut Element, d: &TextBlock) -> Result<(TextBlock, Option<String>)> {
    let (block, note, shift) = lay_out_block(d)?;
    e.transform = e.transform.mul(&Affine::translate(shift.x, shift.y));
    e.kind = ElementKind::Text(Arc::new(block.clone()));
    Ok((block, note))
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    let new_text = opt_str(p, "text")?.map(str::to_string);
    if new_text.as_ref().is_some_and(|t| t.chars().count() > anicraft_text::MAX_CHARS) {
        return Err(EngineError::Param(format!("text is limited to {} characters per block", anicraft_text::MAX_CHARS)));
    }
    let (cp, pp) = (CharPatch::parse(p)?, ParaPatch::parse(p)?);
    let mut notes = Vec::new();
    let mut edited = 0;
    let mut last = None;
    for e in s.selected_elements_mut()? {
        let ElementKind::Text(t) = &e.kind else { continue };
        let text = new_text.clone().unwrap_or_else(|| t.text.clone());
        // Fields may be empty (a script or the viewer fills them); static text may not.
        if text.trim().is_empty() && t.field.is_none() {
            return Err(EngineError::Param("'text' is empty (delete the text instead)".into()));
        }
        // The text changed: the runs follow; then new runs and paragraphs, if given.
        let mut d = retext(t, &text);
        let runs = runs_param(p, &d.char_style())?.unwrap_or_else(|| d.styled_runs());
        let paras = paragraphs_param(p, &d.para_style())?.unwrap_or_else(|| d.paragraph_styles());
        // Style keys apply to every character and paragraph.
        let n = d.char_count();
        let runs = if cp.is_empty() { runs } else { rt::restyle_runs(&runs, 0, n, |st| cp.apply(st)) };
        let paras: Vec<ParagraphStyle> = paras
            .into_iter()
            .map(|mut q| {
                pp.apply(&mut q);
                q
            })
            .collect();
        d.set_styles(runs, paras);
        if t.field.is_some() {
            d.box_width = width_param(p, Some(t.box_width))?.unwrap_or(t.box_width).max(1.0);
        } else {
            d.width = width_param(p, t.width)?;
        }
        let (block, note) = put(e, &d)?;
        notes.extend(note);
        last = Some(block);
        edited += 1;
    }
    if edited == 0 {
        return Err(EngineError::State("the selection has no text".into()));
    }
    if let Some(b) = last {
        remember(s, &b);
    }
    notes.dedup();
    Ok(json!({"edited": edited, "note": notes.first()}))
}

/// `text.style`: characters `start..end` of the selected text restyled.
fn style_range(s: &mut Session, p: &Value) -> Result<Value> {
    let cp = CharPatch::parse(p)?;
    if cp.is_empty() {
        return Err(EngineError::Param(format!("give a style to set: {CHAR_KEYS}")));
    }
    let mut changed = 0;
    let mut notes = Vec::new();
    for e in s.selected_elements_mut()? {
        let ElementKind::Text(t) = &e.kind else { continue };
        let (start, end) = range_param(p, t.char_count())?;
        let mut d = (**t).clone();
        let runs = rt::restyle_runs(&t.styled_runs(), start, end, |st| cp.apply(st));
        d.set_styles(runs, t.paragraph_styles());
        let (_, note) = put(e, &d)?;
        notes.extend(note);
        changed += 1;
    }
    if changed == 0 {
        return Err(EngineError::State("the selection has no text".into()));
    }
    Ok(json!({"changed": changed, "note": notes.first()}))
}

/// `text.paragraph`: the paragraphs characters `start..end` touch, laid out anew.
fn paragraph_range(s: &mut Session, p: &Value) -> Result<Value> {
    let pp = ParaPatch::parse(p)?;
    if pp.is_empty() {
        return Err(EngineError::Param(format!("give a paragraph setting: {PARA_KEYS}")));
    }
    let mut changed = 0;
    for e in s.selected_elements_mut()? {
        let ElementKind::Text(t) = &e.kind else { continue };
        let (start, end) = range_param(p, t.char_count())?;
        let range = rt::paragraphs_in(&t.text, start, end);
        let mut paras = t.paragraph_styles();
        for (i, q) in paras.iter_mut().enumerate() {
            if range.contains(&i) {
                pp.apply(q);
            }
        }
        let mut d = (**t).clone();
        d.set_styles(t.styled_runs(), paras);
        put(e, &d)?;
        changed += 1;
    }
    if changed == 0 {
        return Err(EngineError::State("the selection has no text".into()));
    }
    Ok(json!({"changed": changed}))
}

fn hex_of(c: Rgba) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}

fn style_json(c: &CharStyle) -> Value {
    json!({"font": c.font, "size": c.size, "bold": c.bold, "italic": c.italic, "color": hex_of(c.color), "letter_spacing": c.letter_spacing, "shift": c.shift.key(), "underline": c.underline, "url": c.url})
}

fn align_key(a: TextAlign) -> &'static str {
    match a {
        TextAlign::Left => "left",
        TextAlign::Center => "center",
        TextAlign::Right => "right",
    }
}

fn para_json(q: &ParagraphStyle) -> Value {
    json!({"align": align_key(q.align), "indent": q.indent, "left_margin": q.left_margin, "right_margin": q.right_margin, "line_spacing": q.line_spacing})
}

/// What a summary shares (a mixed attribute is `null`).
pub fn summary_json(m: &StyleSummary) -> Value {
    json!({
        "font": m.font, "size": m.size, "bold": m.bold, "italic": m.italic, "color": m.color.map(hex_of),
        "letter_spacing": m.letter_spacing, "shift": m.shift.map(BaselineShift::key), "underline": m.underline, "url": m.url,
    })
}

/// `text.styles`: the first selected text's runs, paragraphs, and what a range shares.
fn styles_of(s: &mut Session, p: &Value) -> Result<Value> {
    let t = s
        .selected_elements()
        .into_iter()
        .find_map(|(_, e)| match &e.kind {
            ElementKind::Text(t) => Some(t.clone()),
            _ => None,
        })
        .ok_or_else(|| EngineError::State("select a text block".into()))?;
    let (start, end) = range_param(p, t.char_count())?;
    let runs: Vec<Value> = t
        .styled_runs()
        .iter()
        .map(|r| {
            let mut v = style_json(&r.style);
            v["chars"] = json!(r.chars);
            v
        })
        .collect();
    let range = rt::paragraphs_in(&t.text, start, end);
    let paras = t.paragraph_styles();
    let shared = |f: &dyn Fn(&ParagraphStyle) -> Value| -> Value {
        let mut it = paras.iter().enumerate().filter(|(i, _)| range.contains(i)).map(|(_, q)| f(q));
        let first = it.next().unwrap_or(Value::Null);
        if it.all(|v| v == first) { first } else { Value::Null }
    };
    let paragraph = json!({
        "align": shared(&|q| json!(align_key(q.align))),
        "indent": shared(&|q| json!(q.indent)),
        "left_margin": shared(&|q| json!(q.left_margin)),
        "right_margin": shared(&|q| json!(q.right_margin)),
        "line_spacing": shared(&|q| json!(q.line_spacing)),
    });
    Ok(json!({
        "text": t.text,
        "runs": runs,
        "paragraphs": paras.iter().map(para_json).collect::<Vec<_>>(),
        "start": start,
        "end": end,
        "selection": summary_json(&rt::summarize(&t.styled_runs(), start, end)),
        "paragraph": paragraph,
    }))
}

fn fonts(_: &mut Session, _: &Value) -> Result<Value> {
    let f = Fonts::get();
    Ok(json!({"families": f.families(), "default": f.resolve("")}))
}

/// A text block as plain shapes with the same outlines, one per colour (Break Apart).
pub(crate) fn to_shapes(t: &TextBlock) -> Vec<anicraft_doc::Shape> {
    t.fill_paths()
        .into_iter()
        .filter(|(_, _, p)| !p.cmds.is_empty() || t.fills.is_empty())
        .map(|(color, _, path)| anicraft_doc::Shape { path, fill: Some(anicraft_doc::Paint::Solid(color)), stroke: None, fill_rule: anicraft_geom::FillRule::NonZero, merge: false, warp: None, fill_tag: None, stroke_tag: None })
        .collect()
}

/// The one style a block's runtime text is laid out in (its first run's), for fields a script
/// or the viewer changes (ADR 0066).
pub(crate) fn style_of(t: &TextBlock) -> TextStyle {
    TextStyle { font: t.font.clone(), size: t.size, bold: t.bold, italic: t.italic, color: t.color, align: t.align, letter_spacing: t.letter_spacing, line_spacing: t.line_spacing }
}
