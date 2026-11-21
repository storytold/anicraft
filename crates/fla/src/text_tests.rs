//! Text record tests, on records encoded here from the grammar in [`crate::text`] alone (no
//! corpus bytes: the corpus is CC BY-NC-SA).

use crate::text::{self, Alignment, Faces, TextError, CHUNK, GUTTER};
use crate::Report;
use anicraft_doc::{BaselineShift, ElementId, ElementKind, LineType, Rgba, TextAlign, TextKind};
use anicraft_text::Fonts;

/// One run's attributes, as the encoder writes them.
#[derive(Clone)]
pub(crate) struct Style {
    pub(crate) size: u16,
    pub(crate) face: &'static str,
    pub(crate) rgba: [u8; 4],
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) kern: bool,
    pub(crate) position: u8,
    pub(crate) align: u8,
    pub(crate) line: i16,
    pub(crate) indent: i16,
    pub(crate) letter: i16,
    pub(crate) target: &'static str,
    pub(crate) render: u8,
    pub(crate) thickness: f32,
}

impl Default for Style {
    fn default() -> Style {
        Style { size: 240, face: "Arial", rgba: [0, 0, 0, 255], bold: false, italic: false, kern: true, position: 0, align: 0, line: 40, indent: 0, letter: 0, target: "", render: 1, thickness: 0.0 }
    }
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn bare(s: &str) -> Vec<u8> {
    let mut out = vec![s.encode_utf16().count() as u8];
    out.extend(utf16(s));
    out
}

/// An MFC Unicode CString: `FF FE FF <u8 len>`, or `FF FE FF FF <u16 len>` from 255 units.
fn cstring(s: &str) -> Vec<u8> {
    let n = s.encode_utf16().count();
    let mut out = vec![0xFF, 0xFE, 0xFF];
    if n < 0xFF {
        out.push(n as u8);
    } else {
        out.push(0xFF);
        out.extend((n as u16).to_le_bytes());
    }
    out.extend(utf16(s));
    out
}

fn block(s: &Style) -> Vec<u8> {
    let mut out = vec![0x0C];
    out.extend(s.size.to_le_bytes());
    out.extend(bare(s.face));
    out.extend(s.rgba);
    out.extend([0x22, 0x00, s.bold as u8, s.italic as u8, 0, s.kern as u8, s.position, s.align]);
    out.extend(s.line.to_le_bytes());
    out.extend(s.indent.to_le_bytes());
    out.extend([0; 4]);
    out.extend(s.letter.to_le_bytes());
    out.extend([0; 5]);
    out.extend(bare(s.target));
    out.extend([0x02, s.render]);
    out.extend(s.thickness.to_le_bytes());
    out.extend(0f32.to_le_bytes());
    out.push(0);
    out
}

/// A record's head fields.
pub(crate) struct Head {
    pub(crate) flags: u8,
    pub(crate) matrix: [i32; 6],
    pub(crate) bounds: [i32; 4],
    pub(crate) auto: bool,
    pub(crate) field: u16,
    pub(crate) selectable: bool,
    pub(crate) name: String,
}

impl Default for Head {
    fn default() -> Head {
        Head { flags: 0, matrix: [65536, 0, 0, 65536, 2000, 400], bounds: [-100, 4000, 0, 600], auto: false, field: 0, selectable: false, name: String::new() }
    }
}

/// A whole record: head, runs (units and style, stored as given), trailer.
pub(crate) fn record(h: &Head, runs: &[(Vec<u16>, Style)]) -> Vec<u8> {
    let mut out = vec![0x04, h.flags, 0, 0];
    out.extend(0x8000_0000u32.to_le_bytes());
    out.extend(0x8000_0000u32.to_le_bytes());
    out.extend([0, 0, 0x0D]);
    for v in h.matrix.iter().chain(&h.bounds) {
        out.extend(v.to_le_bytes());
    }
    out.extend([h.auto as u8, 0]);
    out.extend(h.field.to_le_bytes());
    out.extend([h.selectable as u8, 0, 0, 0]);
    out.extend(cstring(&h.name));
    if h.field & 0x4000 != 0 {
        out.extend(block(&runs[0].1));
    } else {
        for (units, style) in runs {
            out.extend((units.len() as u16).to_le_bytes());
            out.extend(block(style));
            out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        }
    }
    out.extend([0, 0]);
    out.extend(cstring(""));
    out.extend([0; 8]);
    out.extend(cstring(""));
    out.extend(cstring(""));
    out.extend([0; 3]);
    out
}

pub(crate) fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn fonts() -> Option<&'static Fonts> {
    let f = Fonts::get();
    f.resolve("")?;
    Some(f)
}

#[test]
fn a_static_text_decodes_its_geometry_and_run() {
    let style = Style { size: 280, rgba: [0xBB, 0xBB, 0xBB, 0xFF], align: 2, line: 20, thickness: 58.0, ..Style::default() };
    let h = Head { flags: 0x04, auto: true, bounds: [-109, 8801, 0, 976], ..Head::default() };
    let bytes = record(&h, &[(units("First line\rsecond line"), style)]);
    let t = text::decode(&bytes, 0).unwrap();
    assert_eq!(t.end, bytes.len());
    assert!(t.locked && !t.selected && t.auto_expand);
    assert_eq!((t.kind, t.lines, t.field_flags), (TextKind::Static, LineType::Single, 0));
    assert_eq!(t.matrix.m, [1.0, 0.0, 0.0, 1.0, 100.0, 20.0]);
    assert_eq!((t.left(), t.width(), t.top(), t.height()), (-5.45, 445.5, 0.0, 48.8));
    assert_eq!(t.runs.len(), 1);
    let r = &t.runs[0];
    assert_eq!(r.text, "First line\rsecond line");
    assert_eq!((r.style.size, r.style.face.as_str(), r.style.align, r.style.line_spacing), (14.0, "Arial", Alignment::Center, 1.0));
    assert_eq!(r.style.color(), Rgba::hex(0xBBBBBB));
    assert!(r.style.auto_kern && !r.style.bold && !r.style.italic);
    assert_eq!((t.anti_alias_thickness(), t.standard_rendering()), (Some(58.0), false));
    assert!(t.approximations.is_empty(), "{:?}", t.approximations);
}

#[test]
fn a_dynamic_multiline_field_is_told_from_its_flags() {
    for (field, kind, lines) in [(0x0003, TextKind::Dynamic, LineType::Single), (0x001B, TextKind::Dynamic, LineType::Multiline), (0x0000, TextKind::Static, LineType::Single)] {
        let h = Head { field, ..Head::default() };
        let t = text::decode(&record(&h, &[(units("score"), Style { line: 0, ..Style::default() })]), 0).unwrap();
        assert_eq!((t.kind, t.lines), (kind, lines), "{field:#06x}");
        assert!(t.approximations.is_empty());
    }
}

#[test]
fn a_rotated_and_scaled_matrix_keeps_its_b_and_c_slots() {
    let h = Head { matrix: [0, -81920, 81920, 0, -300, 1234], flags: 0x02, ..Head::default() };
    let t = text::decode(&record(&h, &[(units("Action safe"), Style::default())]), 0).unwrap();
    assert_eq!(t.matrix.m, [0.0, -1.25, 1.25, 0.0, -15.0, 61.7]);
    assert!(t.selected && !t.locked);
}

#[test]
fn runs_keep_their_own_styles() {
    let a = Style { bold: true, rgba: [255, 0, 0, 0x33], size: 640, ..Style::default() };
    let b = Style { italic: true, indent: 520, line: -200, letter: -10, kern: false, ..Style::default() };
    let c = Style { position: 1, align: 1, target: "_blank", render: 0, ..Style::default() };
    let t = text::decode(&record(&Head::default(), &[(units("Bold "), a), (units("italic "), b), (units("2"), c)]), 0).unwrap();
    let styles: Vec<_> = t.runs.iter().map(|r| (r.text.as_str(), r.style.bold, r.style.italic)).collect();
    assert_eq!(styles, [("Bold ", true, false), ("italic ", false, true), ("2", false, false)]);
    assert_eq!((t.runs[0].style.size, t.runs[0].style.color()), (32.0, Rgba::hex(0xFF0000).with_alpha(0x33 as f32 / 255.0)));
    let s = &t.runs[1].style;
    assert_eq!((s.indent, s.line_spacing, s.letter_spacing, s.auto_kern), (26.0, -10.0, -0.5, false));
    let s = &t.runs[2].style;
    assert_eq!((s.shift, s.align, s.target.as_str(), s.standard_rendering), (BaselineShift::Superscript, Alignment::Right, "_blank", true));
    assert!(!t.standard_rendering(), "the record's rendering is its first run's");
}

#[test]
fn the_512_unit_pieces_of_one_run_merge_back() {
    // A 530-unit run of one style, stored as 512 + 18, with a surrogate pair cut by the split.
    let mut long: Vec<u16> = "©".repeat(511).encode_utf16().collect();
    long.extend("😀".encode_utf16()); // units 511 and 512: the split falls inside
    long.extend("tail of the notice".encode_utf16().take(17));
    assert_eq!(long.len(), 530);
    let (first, second) = long.split_at(CHUNK);
    let style = Style::default();
    let other = Style { bold: true, ..Style::default() };
    let runs = [(first.to_vec(), style.clone()), (second.to_vec(), style.clone()), (units(" next"), other.clone()), (units("x"), other)];
    let t = text::decode(&record(&Head::default(), &runs), 0).unwrap();
    let texts: Vec<&str> = t.runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts.len(), 3, "pieces merge; a different style starts a run, and short runs stay apart");
    assert_eq!(texts[0], String::from_utf16_lossy(&long));
    assert!(texts[0].contains('😀'), "the pair survives the split");
    assert_eq!(texts[1..], [" next", "x"]);
}

#[test]
fn a_short_run_is_not_merged_with_the_next_of_its_style() {
    let s = Style::default();
    let t = text::decode(&record(&Head::default(), &[(units("ab"), s.clone()), (units("cd"), s.clone())]), 0).unwrap();
    assert_eq!(t.runs.len(), 2);
    // A full 512-unit piece followed by a differently-styled one stays apart, too.
    let full = vec![u16::from(b'a'); CHUNK];
    let t = text::decode(&record(&Head::default(), &[(full, s), (units("b"), Style { size: 300, ..Style::default() })]), 0).unwrap();
    assert_eq!(t.runs.len(), 2);
}

#[test]
fn an_empty_text_reads_its_one_attribute_block_without_a_length() {
    let h = Head { field: 0x4000, ..Head::default() };
    let style = Style { size: 640, bold: true, kern: false, ..Style::default() };
    let bytes = record(&h, &[(Vec::new(), style)]);
    let t = text::decode(&bytes, 0).unwrap();
    assert_eq!(t.end, bytes.len());
    assert!(t.is_empty());
    assert_eq!(t.runs.len(), 1);
    assert_eq!((t.runs[0].style.size, t.runs[0].style.bold, t.runs[0].style.auto_kern), (32.0, true, false));
    assert_eq!(t.kind, TextKind::Static);
}

#[test]
fn a_long_form_cstring_reads_whole() {
    let name = "n".repeat(300);
    let h = Head { name: name.clone(), ..Head::default() };
    let bytes = record(&h, &[(units("hi"), Style::default())]);
    assert_eq!(&bytes[63..69], &[0xFF, 0xFE, 0xFF, 0xFF, 0x2C, 0x01]);
    let t = text::decode(&bytes, 0).unwrap();
    assert_eq!((t.end, t.runs[0].text.as_str()), (bytes.len(), "hi"));
    assert_eq!(t.approximations, ["text field strings (not decoded yet)"]);
    assert_eq!(text::cstring(&bytes, 63).unwrap(), (name, 69 + 600));
    // The short form, an 8-bit string, and its u16 escape.
    assert_eq!(text::cstring(&cstring("Layer"), 0).unwrap(), ("Layer".to_string(), 14));
    assert_eq!(text::cstring(&[3, b'a', b'b', b'c'], 0).unwrap(), ("abc".to_string(), 4));
    let mut ansi = vec![0xFF, 0x00, 0x01];
    ansi.extend([b'z'; 256]);
    assert_eq!(text::cstring(&ansi, 0).unwrap(), ("z".repeat(256), 259));
    // A u32 length (MFC's next escape) and a doubled Unicode marker.
    let mut wide = vec![0xFF, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 2, 0, 0, 0];
    wide.extend(utf16("ok"));
    assert_eq!(text::cstring(&wide, 0).unwrap(), ("ok".to_string(), 14));
    assert_eq!(text::cstring(&[0xFF, 0xFE, 0xFF, 0xFF, 0xFE, 0xFF, 0], 0), Err(TextError::String));
    assert_eq!(text::cstring(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F], 0), Err(TextError::TooLarge));
}

#[test]
fn unknown_values_degrade_and_are_counted() {
    let h = Head { field: 0x0103 | 0x0008, ..Head::default() };
    let style = Style { position: 7, align: 9, ..Style::default() };
    let t = text::decode(&record(&h, &[(units("odd"), style)]), 0).unwrap();
    assert_eq!((t.kind, t.lines), (TextKind::Dynamic, LineType::Multiline));
    assert_eq!((t.runs[0].style.shift, t.runs[0].style.align), (BaselineShift::Normal, Alignment::Left));
    for what in ["text field settings of an unknown kind (left out)", "text line types of an unknown kind (drawn multiline)", "character positions of an unknown kind (drawn normal)", "text alignments of an unknown kind (drawn left-aligned)"] {
        assert!(t.approximations.contains(&what), "{what}: {:?}", t.approximations);
    }
    // A record whose structure isn't a text's is refused whole.
    let mut bad = record(&Head::default(), &[(units("x"), Style::default())]);
    let run_kind = 63 + 4 + 2;
    bad[run_kind] = 0x0B;
    assert_eq!(text::decode(&bad, 0).unwrap_err(), TextError::RunKind(0x0B));
    assert_eq!(text::decode(&bad, 1).unwrap_err(), TextError::NotText);
    let mut unclosed = record(&Head { field: 0x4000, ..Head::default() }, &[(Vec::new(), Style::default())]);
    let close = 63 + 4 + block(&Style::default()).len();
    unclosed[close] = 1;
    assert_eq!(text::decode(&unclosed, 0).unwrap_err(), TextError::Unclosed);
}

#[test]
fn find_all_walks_records_behind_class_tags() {
    let one = record(&Head::default(), &[(units("one"), Style::default())]);
    let two = record(&Head { field: 0x4000, ..Head::default() }, &[(Vec::new(), Style::default())]);
    let mut stream = vec![0x00, 0x01, 0x02];
    stream.extend([0xFF, 0xFF, 0x01, 0x00, 0x08, 0x00]);
    stream.extend(b"CPicText");
    let first = stream.len();
    stream.extend(&one);
    stream.extend([0x07, 0x80]);
    let second = stream.len();
    stream.extend(&two);
    stream.extend([0x00, 0x00]);
    assert_eq!(text::heads(&stream), [first, second]);
    let found = text::find_all(&stream);
    assert_eq!(found.iter().map(|t| t.head).collect::<Vec<_>>(), [first, second]);
    assert!(found[1].is_empty());
}

#[test]
fn truncated_or_damaged_records_never_panic() {
    let chunked = vec![u16::from(b'c'); CHUNK + 3];
    let samples = [
        record(&Head::default(), &[(units("Hello\rworld"), Style::default()), (units("!"), Style { bold: true, ..Style::default() })]),
        record(&Head { field: 0x4000, ..Head::default() }, &[(Vec::new(), Style::default())]),
        record(&Head { field: 0x001B, name: "x".repeat(260), ..Head::default() }, &[(chunked[..CHUNK].to_vec(), Style::default()), (chunked[CHUNK..].to_vec(), Style::default())]),
    ];
    for bytes in &samples {
        assert!(text::decode(bytes, 0).is_ok());
        for cut in 0..bytes.len() {
            assert!(text::decode(&bytes[..cut], 0).is_err(), "a record cut at {cut} is refused");
            let _ = text::find_all(&bytes[..cut]);
        }
        for at in 0..bytes.len() {
            for v in [0x00, 0x7F, 0xFF] {
                let mut b = bytes.clone();
                b[at] = v;
                let _ = text::decode(&b, 0);
                let _ = text::find_all(&b);
            }
        }
        assert!(text::decode(bytes, usize::MAX).is_err());
    }
}

#[test]
fn a_static_text_becomes_the_xfl_importers_block() {
    let Some(fonts) = fonts() else { return };
    let h = Head { matrix: [131072, 0, 0, 131072, 2000, 1000], bounds: [-100, 4000, 0, 600], ..Head::default() };
    let a = Style { size: 400, rgba: [255, 0, 0, 255], align: 2, letter: 30, line: 80, ..Style::default() };
    let t = text::decode(&record(&h, &[(units("Hello\rWorld"), a)]), 0).unwrap();
    let (mut faces, mut report) = (Faces::new(), Report::default());
    let el = t.to_element(ElementId(7), fonts, &mut faces, &mut report).unwrap();
    let ElementKind::Text(b) = &el.kind else { panic!("a text element") };
    assert_eq!(el.id, ElementId(7));
    assert_eq!(b.text, "Hello\nWorld", "carriage returns are line breaks");
    assert_eq!((b.size, b.color, b.align, b.letter_spacing, b.line_spacing), (20.0, Rgba::hex(0xFF0000), TextAlign::Center, 1.5, 4.0));
    assert_eq!(b.width, Some(205.0), "a fixed-width static text wraps at its bounds");
    assert!(b.field.is_none() && !b.outline.cmds.is_empty());
    // The matrix places the bounds' corner; the text sits inside the gutter.
    assert_eq!(el.transform.m, [2.0, 0.0, 0.0, 2.0, 100.0 + 2.0 * (-5.0 + GUTTER), 50.0 + 2.0 * GUTTER]);
    assert!(report.skipped.is_empty(), "{report:?}");
}

#[test]
fn a_dynamic_field_becomes_a_field_block_and_empty_statics_draw_nothing() {
    let Some(fonts) = fonts() else { return };
    let (mut faces, mut report) = (Faces::new(), Report::default());
    let h = Head { field: 0x001B, bounds: [0, 3000, 0, 800], ..Head::default() };
    let t = text::decode(&record(&h, &[(units("Panel notes"), Style { line: 0, ..Style::default() })]), 0).unwrap();
    let el = t.to_element(ElementId(1), fonts, &mut faces, &mut report).unwrap();
    let ElementKind::Text(b) = &el.kind else { panic!("a text element") };
    let field = b.field.as_ref().expect("a dynamic field");
    assert_eq!((field.kind, field.lines, field.selectable), (TextKind::Dynamic, LineType::Multiline, true));
    assert_eq!((b.box_width, b.box_height, b.text.as_str()), (150.0, 40.0, "Panel notes"));
    assert_eq!(el.transform.m[4..], [100.0, 20.0]);
    assert_eq!(report.runs.get("dynamic text fields"), Some(&1));
    // An empty dynamic field is kept; an empty static text draws nothing.
    let empty = |field| text::decode(&record(&Head { field, ..Head::default() }, &[(Vec::new(), Style::default())]), 0).unwrap();
    assert!(empty(0x4003).to_element(ElementId(2), fonts, &mut faces, &mut report).is_some());
    assert!(empty(0x4000).to_element(ElementId(3), fonts, &mut faces, &mut report).is_none());
    // Justified text is drawn left-aligned and reported, as the XFL importer does.
    let t = text::decode(&record(&Head::default(), &[(units("wide"), Style { align: 3, ..Style::default() })]), 0).unwrap();
    assert!(t.to_element(ElementId(4), fonts, &mut faces, &mut report).is_some());
    assert_eq!(report.skipped.get("justified text (drawn left-aligned)"), Some(&1));
}

#[test]
fn without_fonts_static_text_is_reported_and_fields_stay() {
    let none = Fonts::from_faces(Vec::<Vec<u8>>::new());
    let (mut faces, mut report) = (Faces::new(), Report::default());
    let t = text::decode(&record(&Head::default(), &[(units("Hi"), Style::default())]), 0).unwrap();
    assert!(t.to_element(ElementId(1), &none, &mut faces, &mut report).is_none());
    assert_eq!(report.skipped.get("text fields (no fonts on this computer)"), Some(&1));
    let t = text::decode(&record(&Head { field: 3, ..Head::default() }, &[(units("Hi"), Style::default())]), 0).unwrap();
    assert!(t.to_element(ElementId(2), &none, &mut faces, &mut report).is_some());
}
