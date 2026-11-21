//! `CPicText` records: static and dynamic text fields (ADR 0096), decoded into the same
//! text blocks the XFL importer makes of their twins' `DOMStaticText` and `DOMDynamicText`.
//!
//! Learned differentially from the corpus's legacy files and their Animate twins (every one
//! of the corpus's 372 text records pairs with its twin element, field for field): no Flash
//! or Animate code involved. A record's head is the byte behind its class tag (the class
//! name `CPicText` or a `0x80nn` class reference) and reads, little-endian, offsets from the
//! head:
//!
//! ```text
//! +0   04 · flags (02 selected, 04 locked) · 00 00
//! +4   i32 0x80000000 ×2 (no transformation point) · 00 00
//! +14  0d (the text kind)
//! +15  matrix   a b c d : i32 16.16 · tx ty : i32 twips
//! +39  bounds   xmin xmax ymin ymax : i32 twips (XFL left, width = xmax − xmin, height)
//! +55  autoExpand u8 · 00
//! +57  field flags u16: & 0003 dynamic, & 0018 multiline, & 4000 no characters
//! +59  static isSelectable u8 · 00 00 00
//! +63  CString (empty in the corpus)
//!      runs: { u16 n, attribute block, n UTF-16 units } … u16 0
//!            (with no characters: one attribute block without its n, then u16 0)
//!      trailer: CString · 8 bytes · CString · CString · 3 bytes
//! ```
//!
//! An attribute block (offsets from its first byte; `F` and `T` are the face's and the
//! target's lengths in units):
//!
//! ```text
//! +0       0c · size u16 twips · face (bare u8 length, UTF-16: the family, as `Arial`)
//! q=+5+2F  RGBA · 2 bytes not yet understood · bold · italic · 00 · autoKern
//!          · character position (1 superscript) · alignment (0 left, 1 right, 2 center,
//!          3 justify) · lineSpacing i16 twips · indent i16 twips · 4 bytes (margins?)
//!          · letterSpacing i16 twips · 5 bytes
//! q+27     target (bare u8 length, UTF-16)
//! r        02 · rendering (0: Animate's "standard") · antiAliasThickness f32 · f32 · 00
//! ```
//!
//! A run stores at most 512 units: a longer run of one style is split into 512-unit pieces
//! with byte-identical attribute blocks, which the decode merges back, as XFL shows them.
//! Strings in the header and trailer are MFC `CArchive` strings ([`Reader::cstring`]).

use crate::report::Report;
use anicraft_doc::{BaselineShift, CharStyle, ColorEffect, Element, ElementId, ElementKind, LineType, ParagraphStyle, Rgba, TextAlign, TextBlock, TextField, TextKind, TextRun};
use anicraft_geom::{Affine, Path, Vec2};
use anicraft_text::{Align, FaceMatch, Fonts, TextError as LayoutError};
use std::collections::HashMap;
use std::sync::Arc;

/// The space between a text field's box and its text, on every side (pixels), as in the XFL
/// importer and Animate's text fields.
pub const GUTTER: f32 = 2.0;
/// The bytes from the head to the matrix.
const HEAD: usize = 15;
/// The head every corpus text record starts with: `04`, any flag byte, then fixed bytes up to
/// the `0d` kind (index 1 is not compared).
const SIGNATURE: [u8; HEAD] = [0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x0D];
/// The first byte of every run's attribute block.
const RUN_KIND: u8 = 0x0C;
/// The longest run a record stores; longer runs of one style are split into pieces this long.
pub const CHUNK: usize = 512;
/// Field flags: a dynamic field (either bit).
const DYNAMIC: u16 = 0x0003;
/// Field flags: a multiline dynamic field (both bits in the corpus).
const MULTILINE: u16 = 0x0018;
/// Field flags: a text without characters, whose one attribute block has no length.
const NO_CHARACTERS: u16 = 0x4000;
/// Every field flag seen in the corpus.
const KNOWN_FLAGS: u16 = DYNAMIC | MULTILINE | NO_CHARACTERS;
/// The most runs one record may store.
pub const MAX_STORED_RUNS: usize = 1 << 16;
/// The most UTF-16 units one record may store.
pub const MAX_UNITS: usize = 1 << 22;
/// The longest CString read, in units.
pub const MAX_STRING: usize = 1 << 20;

/// Why a text record didn't decode: the record is left out whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    #[error("not a text record")]
    NotText,
    #[error("the record ends early")]
    Truncated,
    #[error("a run's attribute block of kind {0:#04x}")]
    RunKind(u8),
    #[error("a text without characters whose attribute block isn't closed")]
    Unclosed,
    #[error("a string in the record isn't one")]
    String,
    #[error("more runs, characters or string units than a text may hold")]
    TooLarge,
}

/// How a run's characters line up in their paragraph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    Left,
    Right,
    Center,
    Justify,
}

/// One run's style, as stored (sizes and spacings in pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct RunStyle {
    pub size: f32,
    /// The font family (bold and italic are separate).
    pub face: String,
    /// RGBA, as stored.
    pub rgba: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub auto_kern: bool,
    pub shift: BaselineShift,
    pub align: Alignment,
    pub line_spacing: f32,
    pub indent: f32,
    pub letter_spacing: f32,
    /// The link's target window (`_blank`), stored without a link in the corpus.
    pub target: String,
    /// Animate's "standard" font rendering (the stored byte is 0).
    pub standard_rendering: bool,
    /// The anti-alias thickness, 0 when unset.
    pub anti_alias_thickness: f32,
}

impl RunStyle {
    /// The colour, as the XFL importer reads `fillColor` and `alpha`.
    pub fn color(&self) -> Rgba {
        let [r, g, b, a] = self.rgba;
        Rgba::hex(u32::from_be_bytes([0, r, g, b])).with_alpha(a as f32 / 255.0)
    }
}

/// A run of characters in one style (the 512-unit pieces merged back).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: RunStyle,
}

/// A decoded text record.
#[derive(Clone, Debug)]
pub struct TextRecord {
    /// The head's offset in its stream.
    pub head: usize,
    pub selected: bool,
    pub locked: bool,
    pub matrix: Affine,
    /// xmin, xmax, ymin, ymax in twips.
    pub bounds: [i32; 4],
    pub auto_expand: bool,
    /// The field flags as stored.
    pub field_flags: u16,
    /// Static or dynamic (input text is not in the corpus, so never decoded as such).
    pub kind: TextKind,
    /// A dynamic field's line type (`Single` for static text).
    pub lines: LineType,
    /// The static-text "selectable" byte (dynamic fields store 0 and are selectable).
    pub selectable: bool,
    /// The runs; a text without characters has one, empty, carrying its style.
    pub runs: Vec<Run>,
    /// What was read the nearest way AniCraft can, or left out, as report kinds.
    pub approximations: Vec<&'static str>,
    /// Where the record ended (the byte after its trailer).
    pub end: usize,
}

/// Bounded little-endian reads: every read past the end is [`TextError::Truncated`].
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], TextError> {
        let end = self.at.checked_add(n).ok_or(TextError::Truncated)?;
        let s = self.data.get(self.at..end).ok_or(TextError::Truncated)?;
        self.at = end;
        Ok(s)
    }
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], TextError> {
        <[u8; N]>::try_from(self.take(N)?).map_err(|_| TextError::Truncated)
    }
    fn u8(&mut self) -> Result<u8, TextError> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, TextError> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }
    fn i16(&mut self) -> Result<i16, TextError> {
        Ok(i16::from_le_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32, TextError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn i32(&mut self) -> Result<i32, TextError> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }
    fn f32(&mut self) -> Result<f32, TextError> {
        Ok(f32::from_le_bytes(self.bytes()?))
    }
    /// `n` UTF-16LE units.
    fn units(&mut self, n: usize) -> Result<Vec<u16>, TextError> {
        let bytes = self.take(n.checked_mul(2).ok_or(TextError::TooLarge)?)?;
        Ok(bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect())
    }
    /// A matrix: a, b, c, d in 16.16 fixed point, then tx, ty in twips.
    fn matrix(&mut self) -> Result<Affine, TextError> {
        let mut m = [0f32; 6];
        for (i, v) in m.iter_mut().enumerate() {
            let raw = self.i32()? as f32;
            *v = if i < 4 { raw / 65536.0 } else { raw / 20.0 };
        }
        Ok(Affine { m })
    }
    /// A run's string: a bare u8 length, then UTF-16LE units.
    fn bare_string(&mut self) -> Result<String, TextError> {
        let n = self.u8()? as usize;
        Ok(String::from_utf16_lossy(&self.units(n)?))
    }
    /// An MFC `CArchive` string: a u8 length; `FF` escapes to a u16 length, and `FFFF` to a
    /// u32 one. A u16 of `FFFE` marks the string as UTF-16 and its length follows again, so
    /// the corpus's strings read `FF FE FF <u8>` and, from 255 units, `FF FE FF FF <u16>`.
    /// Unmarked strings are 8-bit (read as Latin-1).
    fn cstring(&mut self) -> Result<String, TextError> {
        let mut wide = false;
        let len = loop {
            let b = self.u8()?;
            if b < 0xFF {
                break b as usize;
            }
            let w = self.u16()?;
            if w == 0xFFFE {
                if wide {
                    return Err(TextError::String);
                }
                wide = true;
                continue;
            }
            if w < 0xFFFF {
                break w as usize;
            }
            break usize::try_from(self.u32()?).map_err(|_| TextError::TooLarge)?;
        };
        if len > MAX_STRING {
            return Err(TextError::TooLarge);
        }
        Ok(if wide { String::from_utf16_lossy(&self.units(len)?) } else { self.take(len)?.iter().map(|&b| b as char).collect() })
    }
}

/// Reads the MFC `CArchive` string at `at` (see [`decode`]'s header strings): its text and
/// the offset after it. Other records carry such strings too (a component instance's
/// metadata XML), so the object walker can skip them whole.
pub fn cstring(data: &[u8], at: usize) -> Result<(String, usize), TextError> {
    let mut r = Reader { data, at };
    let s = r.cstring()?;
    Ok((s, r.at))
}

fn note(approximations: &mut Vec<&'static str>, what: &'static str) {
    if !approximations.contains(&what) {
        approximations.push(what);
    }
}

fn px(twips: i16) -> f32 {
    twips as f32 / 20.0
}

/// One attribute block, and its bytes (pieces of one run have byte-identical blocks).
fn block<'a>(r: &mut Reader<'a>, approximations: &mut Vec<&'static str>) -> Result<(RunStyle, &'a [u8]), TextError> {
    let start = r.at;
    let kind = r.u8()?;
    if kind != RUN_KIND {
        return Err(TextError::RunKind(kind));
    }
    let size = r.u16()? as f32 / 20.0;
    let face = r.bare_string()?;
    let rgba: [u8; 4] = r.bytes()?;
    r.bytes::<2>()?; // vary between files; no XFL attribute follows them
    let [bold, italic, zero, auto_kern, position, align] = r.bytes::<6>()?;
    let line_spacing = px(r.i16()?);
    let indent = px(r.i16()?);
    let margins: [u8; 4] = r.bytes()?;
    let letter_spacing = px(r.i16()?);
    let rest: [u8; 5] = r.bytes()?;
    let target = r.bare_string()?;
    let [_aa_mode, rendering] = r.bytes::<2>()?; // the first is 02 throughout the corpus
    let thickness = r.f32()?;
    r.f32()?; // zero throughout the corpus (the anti-alias sharpness?)
    let end_zero = r.u8()?;
    if margins != [0; 4] {
        note(approximations, "text margins (not decoded yet)");
    }
    if zero != 0 || rest != [0; 5] || end_zero != 0 {
        note(approximations, "text attributes not decoded yet");
    }
    let shift = match position {
        0 => BaselineShift::Normal,
        1 => BaselineShift::Superscript,
        // Not in the corpus; the next value after superscript, as in XFL's list.
        2 => BaselineShift::Subscript,
        _ => {
            note(approximations, "character positions of an unknown kind (drawn normal)");
            BaselineShift::Normal
        }
    };
    let align = match align {
        0 => Alignment::Left,
        1 => Alignment::Right,
        2 => Alignment::Center,
        3 => Alignment::Justify,
        _ => {
            note(approximations, "text alignments of an unknown kind (drawn left-aligned)");
            Alignment::Left
        }
    };
    let style = RunStyle {
        size,
        face,
        rgba,
        bold: bold != 0,
        italic: italic != 0,
        auto_kern: auto_kern != 0,
        shift,
        align,
        line_spacing,
        indent,
        letter_spacing,
        target,
        standard_rendering: rendering == 0,
        anti_alias_thickness: if thickness.is_finite() { thickness } else { 0.0 },
    };
    Ok((style, r.data.get(start..r.at).unwrap_or_default()))
}

/// Decodes the text record whose head (the `04` byte behind its class tag) is at `at`.
pub fn decode(data: &[u8], at: usize) -> Result<TextRecord, TextError> {
    let mut r = Reader { data, at };
    let head: [u8; HEAD] = r.bytes()?;
    if head[0] != 0x04 || head[2..4] != [0, 0] || head[12..14] != [0, 0] || head[14] != 0x0D {
        return Err(TextError::NotText);
    }
    let mut approximations = Vec::new();
    let matrix = r.matrix()?;
    let bounds = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
    let [auto_expand, _] = r.bytes::<2>()?;
    let field_flags = r.u16()?;
    let [selectable, _, _, _] = r.bytes::<4>()?;
    if field_flags & !KNOWN_FLAGS != 0 {
        note(&mut approximations, "text field settings of an unknown kind (left out)");
    }
    let kind = if field_flags & DYNAMIC != 0 { TextKind::Dynamic } else { TextKind::Static };
    let lines = match field_flags & MULTILINE {
        0 => LineType::Single,
        MULTILINE => LineType::Multiline,
        _ => {
            note(&mut approximations, "text line types of an unknown kind (drawn multiline)");
            LineType::Multiline
        }
    };
    if !r.cstring()?.is_empty() {
        note(&mut approximations, "text field strings (not decoded yet)");
    }
    // The stored runs, the 512-unit pieces of one run merged back as they come.
    let mut merged: Vec<(Vec<u16>, RunStyle, &[u8])> = Vec::new();
    if field_flags & NO_CHARACTERS != 0 {
        let (style, raw) = block(&mut r, &mut approximations)?;
        if r.u16()? != 0 {
            return Err(TextError::Unclosed);
        }
        merged.push((Vec::new(), style, raw));
    } else {
        let (mut stored, mut total) = (0usize, 0usize);
        loop {
            let n = r.u16()? as usize;
            if n == 0 {
                break;
            }
            stored += 1;
            total += n;
            if stored > MAX_STORED_RUNS || total > MAX_UNITS {
                return Err(TextError::TooLarge);
            }
            let (style, raw) = block(&mut r, &mut approximations)?;
            let units = r.units(n)?;
            match merged.last_mut() {
                Some(last) if !last.0.is_empty() && last.0.len() % CHUNK == 0 && last.2 == raw => last.0.extend(units),
                _ => merged.push((units, style, raw)),
            }
        }
    }
    // The trailer: three CStrings around fixed bytes, empty or zero in the corpus.
    let named = !r.cstring()?.is_empty();
    let fixed: [u8; 8] = r.bytes()?;
    let named = named | !r.cstring()?.is_empty() | !r.cstring()?.is_empty();
    let tail: [u8; 3] = r.bytes()?;
    if named {
        note(&mut approximations, "text field strings (not decoded yet)");
    }
    if fixed != [0; 8] || tail != [0; 3] {
        note(&mut approximations, "text field settings of an unknown kind (left out)");
    }
    let runs = merged.into_iter().map(|(units, style, _)| Run { text: String::from_utf16_lossy(&units), style }).collect();
    Ok(TextRecord {
        head: at,
        selected: head[1] & 0x02 != 0,
        locked: head[1] & 0x04 != 0,
        matrix,
        bounds,
        auto_expand: auto_expand != 0,
        field_flags,
        kind,
        lines,
        selectable: selectable != 0,
        runs,
        approximations,
        end: r.at,
    })
}

/// The offsets of every byte run in a stream that has a text record's head (the 15-byte
/// signature, any flag byte): candidates for [`decode`].
pub fn heads(data: &[u8]) -> Vec<usize> {
    data.windows(HEAD).enumerate().filter(|(_, w)| w[0] == SIGNATURE[0] && w[2..] == SIGNATURE[2..]).map(|(i, _)| i).collect()
}

/// Every text record in a stream that decodes whole, found by its head and checked by a
/// full decode. For inspection and differential tests: the record-level pass that places
/// them on frames calls [`decode`] at each text object's head.
pub fn find_all(data: &[u8]) -> Vec<TextRecord> {
    let mut out: Vec<TextRecord> = Vec::new();
    for at in heads(data) {
        if out.last().is_some_and(|last| at < last.end) {
            continue;
        }
        if let Ok(record) = decode(data, at) {
            out.push(record);
        }
    }
    out
}

/// The faces looked up so far, by name (one lookup per face per import).
pub type Faces = HashMap<String, FaceMatch>;

/// A record's text with its runs (faces resolved to installed families) and one style per
/// paragraph, as the XFL importer reads a field's runs.
struct Styled {
    text: String,
    runs: Vec<TextRun>,
    paragraphs: Vec<ParagraphStyle>,
    missing: Vec<String>,
    justified: bool,
}

impl TextRecord {
    /// Whether the record holds no characters.
    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.text.is_empty())
    }
    /// XFL's `left`, `top`, `width` and `height`, in pixels.
    pub fn left(&self) -> f32 {
        self.bounds[0] as f32 / 20.0
    }
    pub fn top(&self) -> f32 {
        self.bounds[2] as f32 / 20.0
    }
    pub fn width(&self) -> f32 {
        (self.bounds[1] as f64 - self.bounds[0] as f64) as f32 / 20.0
    }
    pub fn height(&self) -> f32 {
        (self.bounds[3] as f64 - self.bounds[2] as f64) as f32 / 20.0
    }
    /// XFL's `fontRenderingMode="standard"` (stored per run; XFL keeps the first run's).
    pub fn standard_rendering(&self) -> bool {
        self.runs.first().is_some_and(|r| r.style.standard_rendering)
    }
    /// XFL's `antiAliasThickness`, when set (stored per run; XFL keeps the first run's).
    pub fn anti_alias_thickness(&self) -> Option<f32> {
        self.runs.first().map(|r| r.style.anti_alias_thickness).filter(|t| *t != 0.0)
    }

    fn styled(&self, fonts: &Fonts, faces: &mut Faces, report: &mut Report) -> Styled {
        let mut text = String::new();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut para_of_run: Vec<ParagraphStyle> = Vec::new();
        let (mut chars, mut missing, mut justified) = (0usize, Vec::new(), false);
        let mut cut = self.runs.len() > anicraft_doc::MAX_TEXT_RUNS;
        for run in self.runs.iter().take(anicraft_doc::MAX_TEXT_RUNS) {
            let s = &run.style;
            let align = match s.align {
                Alignment::Left => TextAlign::Left,
                Alignment::Right => TextAlign::Right,
                Alignment::Center => TextAlign::Center,
                Alignment::Justify => {
                    justified = true;
                    TextAlign::Left
                }
            };
            let name: String = s.face.chars().take(256).collect();
            let face = faces.entry(name.clone()).or_insert_with(|| fonts.find_face(&name)).clone();
            if face.family.is_none() && !face.wanted.is_empty() && !missing.contains(&face.wanted) {
                missing.push(face.wanted.clone());
            }
            let finite = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
            let c = CharStyle {
                font: face.family.clone().unwrap_or_default(),
                size: s.size.clamp(1.0, 2000.0),
                bold: s.bold | face.bold,
                italic: s.italic | face.italic,
                color: s.color(),
                letter_spacing: finite(s.letter_spacing, -1000.0, 1000.0),
                shift: s.shift,
                underline: false,
                url: String::new(),
            };
            let p = ParagraphStyle { align, indent: finite(s.indent, -1000.0, 1000.0), left_margin: 0.0, right_margin: 0.0, line_spacing: finite(s.line_spacing, -1000.0, 1000.0) };
            // Animate ends lines with a carriage return; any line break starts a new line here.
            let piece = run.text.replace("\r\n", "\n").replace(['\r', '\u{2028}', '\u{2029}'], "\n");
            let room = anicraft_text::MAX_CHARS.saturating_sub(chars);
            let n = piece.chars().count();
            cut |= n > room;
            let n = n.min(room);
            text.extend(piece.chars().take(n));
            chars += n;
            runs.push(TextRun { chars: n as u32, style: c });
            para_of_run.push(p);
            if chars >= anicraft_text::MAX_CHARS {
                break;
            }
        }
        if cut {
            report.skip("text cut to 100,000 characters per field");
        }
        // Each paragraph takes the paragraph attributes of the run its first character is in.
        let mut starts = vec![0usize];
        starts.extend(text.chars().enumerate().filter(|(_, c)| *c == '\n').map(|(i, _)| i + 1));
        let mut ends: Vec<usize> = Vec::with_capacity(runs.len());
        let mut at = 0usize;
        for r in &runs {
            at += r.chars as usize;
            ends.push(at);
        }
        let paragraphs = starts.into_iter().map(|s| {
            let k = ends.partition_point(|e| *e <= s).min(para_of_run.len().saturating_sub(1));
            para_of_run.get(k).copied().unwrap_or_default()
        }).collect();
        Styled { text, runs, paragraphs, missing, justified }
    }

    /// The record as a text block and where its box's top-left goes in the record's matrix
    /// space, laid out with `fonts` exactly as the XFL importer lays out the twin element:
    /// static text as wrapped (fixed width) or auto-size text inside the gutter, dynamic text
    /// as a field (ADR 0066). `None` for static text with nothing to draw (the corpus's
    /// empty statics), or when the computer has no fonts (reported).
    pub fn block(&self, fonts: &Fonts, faces: &mut Faces, report: &mut Report) -> Option<(TextBlock, Vec2)> {
        let styled = self.styled(fonts, faces, report);
        if styled.justified {
            report.skip("justified text (drawn left-aligned)");
        }
        let width = Some(self.width()).filter(|w| *w > 0.0 && w.is_finite()).map(|w| w.min(100_000.0));
        if self.kind != TextKind::Static {
            return self.field(fonts, report, styled, width);
        }
        // Trailing empty lines draw nothing.
        let keep = styled.text.trim_end().chars().count();
        if keep == 0 || styled.runs.is_empty() {
            return None;
        }
        let text: String = styled.text.chars().take(keep).collect();
        // A fixed-width field wraps inside its margins; an auto-size one never wraps.
        let width = if self.auto_expand { None } else { width };
        let mut block = TextBlock { text: text.clone(), width, ..TextBlock::default() };
        block.set_styles(styled.runs, styled.paragraphs);
        match lay_out(fonts, &mut block, &text, width) {
            Ok(l) => {
                report.missing_fonts.extend(styled.missing);
                block.box_width = l.width;
                block.box_height = l.height;
            }
            Err(LayoutError::NoFonts) => {
                report.skip("text fields (no fonts on this computer)");
                return None;
            }
            Err(LayoutError::TooLong) => {
                report.skip("text cut to 100,000 characters per field");
                return None;
            }
        }
        Some((block, Vec2::new(self.left() + GUTTER, self.top() + GUTTER)))
    }

    /// A dynamic field: one block with the record's runs, its box from the bounds (or the
    /// text's size, for an auto-size field), its text laid out inside the gutter. Empty
    /// fields are kept.
    fn field(&self, fonts: &Fonts, report: &mut Report, styled: Styled, width: Option<f32>) -> Option<(TextBlock, Vec2)> {
        *report.runs.entry("dynamic text fields".into()).or_default() += 1;
        let keep = styled.text.trim_end_matches('\n').chars().count();
        let text: String = styled.text.chars().take(keep).collect();
        // Dynamic fields store 0 in the static "selectable" byte; Animate's default holds.
        let field = TextField { lines: self.lines, selectable: true, ..TextField::new(self.kind) };
        let mut block = TextBlock { text: text.clone(), ..TextBlock::default() };
        block.set_styles(styled.runs, styled.paragraphs);
        let wrap = width.and_then(|w| field.wrap_width(w));
        // A fixed box that doesn't wrap still aligns its lines in the box.
        let lay_width = wrap.or(if self.auto_expand { None } else { width.map(|w| (w - 2.0 * GUTTER).max(1.0)).filter(|_| block.align != TextAlign::Left) });
        let shown = field.shown(&text);
        let (tw, th) = match lay_out(fonts, &mut block, &shown, lay_width) {
            Ok(l) => {
                report.missing_fonts.extend(styled.missing);
                let to = Affine::translate(GUTTER, GUTTER);
                let pieces: Vec<(Rgba, String, Path)> = block.fill_paths().into_iter().map(|(c, u, p)| (c, u.to_string(), p.transformed(&to))).collect();
                block.set_outline(pieces);
                (l.width, l.height)
            }
            Err(e) => {
                report.skip(if e == LayoutError::NoFonts { "text fields (no fonts on this computer)" } else { "text cut to 100,000 characters per field" });
                block.outline = Arc::new(Path::new());
                (0.0, block.size * 1.2)
            }
        };
        block.box_width = match (width, self.auto_expand) {
            (Some(w), false) => w,
            (Some(w), true) => w.max(tw + 2.0 * GUTTER),
            (None, _) => tw + 2.0 * GUTTER,
        };
        let height = self.height();
        block.box_height = if height > 0.0 && height.is_finite() { height.min(100_000.0) } else { th + 2.0 * GUTTER };
        block.width = wrap;
        block.field = Some(field);
        Some((block, Vec2::new(self.left(), self.top())))
    }

    /// The record as a text element with the given id, placed by its matrix (premultiply a
    /// parent's transform for texts inside groups). Reports what it approximated, as the
    /// record's report kinds, plus anything [`TextRecord::block`] reports.
    pub fn to_element(&self, id: ElementId, fonts: &Fonts, faces: &mut Faces, report: &mut Report) -> Option<Element> {
        for what in &self.approximations {
            report.skip(what);
        }
        let (block, offset) = self.block(fonts, faces, report)?;
        Some(Element { id, name: String::new(), transform: self.matrix.mul(&Affine::translate(offset.x, offset.y)), color: ColorEffect::default(), kind: ElementKind::Text(Arc::new(block)) })
    }
}

/// Lays a block out from its text, runs, paragraphs and width: the XFL importer's
/// (`anicraft-xfl`'s `text::lay_out`), which sits on the same layer, so it is mirrored here.
fn lay_out(fonts: &Fonts, block: &mut TextBlock, shown: &str, wrap: Option<f32>) -> Result<anicraft_text::Layout, LayoutError> {
    let mut paints: Vec<(Rgba, String)> = Vec::new();
    let align = |a: TextAlign| match a {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    };
    let runs: Vec<anicraft_text::Run> = block
        .styled_runs()
        .into_iter()
        .map(|r| {
            let key = (r.style.color, r.style.url.clone());
            let paint = paints.iter().position(|k| *k == key).unwrap_or_else(|| {
                paints.push(key);
                paints.len() - 1
            }) as u32;
            let shift = match r.style.shift {
                BaselineShift::Normal => anicraft_text::Shift::Normal,
                BaselineShift::Superscript => anicraft_text::Shift::Superscript,
                BaselineShift::Subscript => anicraft_text::Shift::Subscript,
            };
            anicraft_text::Run { chars: r.chars as usize, style: anicraft_text::RunStyle { family: r.style.font, size: r.style.size, bold: r.style.bold, italic: r.style.italic, letter_spacing: r.style.letter_spacing, shift, underline: r.style.underline, paint } }
        })
        .collect();
    let paras: Vec<anicraft_text::ParaStyle> = block.paragraph_styles().into_iter().map(|p| anicraft_text::ParaStyle { align: align(p.align), indent: p.indent, left_margin: p.left_margin, right_margin: p.right_margin, line_spacing: p.line_spacing }).collect();
    let l = fonts.layout_rich(&anicraft_text::RichText { text: shown, runs: &runs, paragraphs: &paras, width: wrap }, false)?;
    // The runs record the families actually used.
    let mut styled = block.styled_runs();
    for (r, used) in styled.iter_mut().zip(&l.families) {
        r.style.font.clone_from(used);
    }
    let p = block.paragraph_styles();
    block.set_styles(styled, p);
    let pieces = l.pieces.iter().map(|(k, p)| paints.get(*k as usize).map_or((block.color, String::new(), p.clone()), |(c, u)| (*c, u.clone(), p.clone()))).collect();
    block.set_outline(pieces);
    Ok(l)
}
