//! Styled text (ADR 0085): runs set on a range, typing that continues a style, paragraphs, mixed
//! selections, fields and paths keeping runs, Break Apart by colour, the file, the renderer.

use crate::Session;
use anicraft_doc::{BaselineShift, ElementId, ElementKind, TextAlign, TextBlock};
use serde_json::{Value, json};
use std::sync::Arc;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn fonts() -> bool {
    let ok = !anicraft_text::Fonts::get().families().is_empty();
    if !ok {
        eprintln!("no fonts on this computer: styled text tests skipped");
    }
    ok
}

fn text_of(s: &Session, id: ElementId) -> Arc<TextBlock> {
    let e = s.visible_elements().into_iter().find(|e| e.id == id).unwrap();
    match &e.kind {
        ElementKind::Text(t) => t.clone(),
        other => panic!("not text: {other:?}"),
    }
}

fn hello(s: &mut Session) -> ElementId {
    ElementId(run(s, "text.create", json!({"text": "Hello world", "x": 20, "y": 20, "size": 40, "color": "#000000"}))["element"].as_u64().unwrap())
}

#[test]
fn styling_a_range_splits_runs_and_colours_the_outline() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let id = hello(&mut s);
    let plain = text_of(&s, id);
    assert!(plain.runs.is_empty() && plain.fills.is_empty(), "one style: as text always was");
    let steps = s.history().undo_len();
    run(&mut s, "text.style", json!({"start": 0, "end": 5, "bold": true, "color": "#cc0000"}));
    assert_eq!(s.history().undo_len(), steps + 1, "one undo step");
    let t = text_of(&s, id);
    assert_eq!(t.runs.iter().map(|r| (r.chars, r.style.bold)).collect::<Vec<_>>(), vec![(5, true), (6, false)]);
    assert_eq!(t.fills.len(), 2, "a piece per colour");
    assert_eq!(t.fills.iter().map(|f| f.cmds as usize).sum::<usize>(), t.outline.cmds.len());
    assert_eq!(t.fill_paths()[0].0, anicraft_doc::Rgba::hex(0xcc0000));
    // The block's own style is its first run's.
    assert!(t.bold && t.color == anicraft_doc::Rgba::hex(0xcc0000));
    // The renderer draws the first word red and the second black.
    let px = anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap();
    let count = |want: fn(u8, u8, u8) -> bool| {
        let raw = px.to_rgba8();
        raw.as_chunks::<4>().0.iter().filter(|p| want(p[0], p[1], p[2])).count()
    };
    assert!(count(|r, g, b| r > 150 && g < 60 && b < 60) > 50, "red glyphs drawn");
    assert!(count(|r, g, b| r < 40 && g < 40 && b < 40) > 50, "black glyphs drawn");
    // Restyling it back joins the runs again.
    run(&mut s, "text.style", json!({"start": 0, "end": 5, "bold": false, "color": "#000000"}));
    let back = text_of(&s, id);
    assert!(back.runs.is_empty() && back.fills.is_empty());
    assert_eq!(back.outline, plain.outline, "laid out exactly as before");
}

#[test]
fn styles_report_mixed_values_and_typing_continues_the_style_before() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let id = hello(&mut s);
    run(&mut s, "text.style", json!({"start": 6, "end": 11, "size": 20, "italic": true}));
    let v = run(&mut s, "text.styles", json!({"start": 3, "end": 8}));
    assert_eq!(v["selection"]["size"], Value::Null, "mixed");
    assert_eq!(v["selection"]["bold"], json!(false), "shared");
    assert_eq!(v["runs"].as_array().unwrap().len(), 2);
    assert_eq!(run(&mut s, "text.styles", json!({"start": 9, "end": 9}))["selection"]["italic"], json!(true), "a caret: the style typing there gets");
    // Typing at the end continues the small italic run; typing at the start the first run.
    run(&mut s, "text.edit", json!({"text": "Hello world!"}));
    run(&mut s, "text.edit", json!({"text": ">Hello world!"}));
    let t = text_of(&s, id);
    assert_eq!(t.runs.iter().map(|r| (r.chars, r.style.size)).collect::<Vec<_>>(), vec![(7, 40.0), (6, 20.0)]);
    // Deleting the small word joins nothing it shouldn't; deleting all of it leaves one run.
    run(&mut s, "text.edit", json!({"text": ">Hello "}));
    assert!(text_of(&s, id).runs.is_empty());
    // Hostile ranges and junk styles are errors or clamps, never a panic.
    assert!(s.execute("text.style", &json!({"start": 4, "end": 2, "bold": true})).is_err());
    assert!(s.execute("text.style", &json!({"start": 0})).is_err(), "no style given");
    assert!(s.execute("text.style", &json!({"shift": "sideways"})).is_err());
    assert!(s.execute("text.style", &json!({"url": "javascript:alert(1)"})).is_err());
    run(&mut s, "text.style", json!({"start": 3, "end": 999_999, "underline": true}));
}

#[test]
fn a_superscript_sits_above_the_line_and_runs_wrap_across() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let id = ElementId(run(&mut s, "text.create", json!({"text": "E = mc2", "x": 0, "y": 0, "size": 60}))["element"].as_u64().unwrap());
    let before = text_of(&s, id);
    run(&mut s, "text.style", json!({"start": 6, "end": 7, "shift": "superscript"}));
    let t = text_of(&s, id);
    assert_eq!(t.runs.last().map(|r| r.style.shift), Some(BaselineShift::Superscript));
    // The "2" moved up and got smaller: the outline's top is no lower, the text narrower.
    assert!(t.box_width < before.box_width - 5.0, "{} vs {}", t.box_width, before.box_width);
    assert!(t.outline.bounds().min.y <= before.outline.bounds().min.y + 0.5);
    // Wrapping across runs: a width that fits two words per line.
    run(
        &mut s,
        "text.edit",
        json!({"text": "one two three four five", "width": 180, "runs": [{"chars": 8, "size": 30}, {"chars": 15, "size": 60, "color": "#0000ff"}]}),
    );
    let w = text_of(&s, id);
    assert!(w.box_height > 150.0, "several lines: {}", w.box_height);
    assert!(w.outline.bounds().max.x <= 182.0, "{:?}", w.outline.bounds());
    assert_eq!(w.runs.len(), 2);
}

#[test]
fn paragraphs_take_their_own_alignment_indent_and_margins() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let id = ElementId(
        run(&mut s, "text.create", json!({"text": "Title\nbody text", "x": 0, "y": 0, "size": 30, "width": 400}))["element"].as_u64().unwrap(),
    );
    run(&mut s, "text.paragraph", json!({"start": 0, "end": 0, "align": "center"}));
    let t = text_of(&s, id);
    assert_eq!(t.paragraphs.iter().map(|p| p.align).collect::<Vec<_>>(), vec![TextAlign::Center, TextAlign::Left]);
    let v = run(&mut s, "text.styles", json!({"start": 2, "end": 9}));
    assert_eq!(v["paragraph"]["align"], Value::Null, "two paragraphs, two alignments");
    run(&mut s, "text.paragraph", json!({"start": 7, "end": 7, "indent": 40, "left_margin": 10}));
    let t = text_of(&s, id);
    assert_eq!((t.paragraphs[1].indent, t.paragraphs[1].left_margin), (40.0, 10.0));
    // A line break typed in the centred title gives both halves its alignment.
    run(&mut s, "text.edit", json!({"text": "Ti\ntle\nbody text"}));
    assert_eq!(text_of(&s, id).paragraphs.iter().map(|p| p.align).collect::<Vec<_>>(), vec![TextAlign::Center, TextAlign::Center, TextAlign::Left]);
    assert!(s.execute("text.paragraph", &json!({"indent": 1e6})).is_err());
    assert!(s.execute("text.paragraph", &json!({"align": "justify"})).is_err());
}

#[test]
fn runs_survive_the_file_fields_paths_and_break_apart() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let id = hello(&mut s);
    run(&mut s, "text.style", json!({"start": 0, "end": 5, "color": "#00aa00", "url": "https://example.org"}));
    // The file keeps runs, paragraphs and fills.
    let json = crate::format::to_json(&s.doc).unwrap();
    let back = crate::format::from_json(&json).unwrap();
    let mut s2 = Session::new(back);
    s2.frame = 0;
    let t2 = text_of(&s2, id);
    let t = text_of(&s, id);
    assert_eq!((&t2.runs, &t2.fills, &t2.outline), (&t.runs, &t.fills, &t.outline));
    // Links reach the display list for exports.
    let dl = anicraft_doc::evaluate(&s.doc, 0);
    assert_eq!(dl.links.len(), 1);
    assert_eq!(&*dl.links[0].1, "https://example.org");
    // A field keeps its runs inside its box.
    run(&mut s, "text.field", json!({"type": "dynamic"}));
    let f = text_of(&s, id);
    assert!(f.field.is_some() && f.runs.len() == 2 && f.fills.len() == 2);
    run(&mut s, "text.field", json!({"type": "static"}));
    // On a path, the colours stay.
    let line = ElementId(run(&mut s, "draw.line", json!({"x0": 100, "y0": 300, "x1": 600, "y1": 300}))["element"].as_u64().unwrap());
    run(&mut s, "element.select", json!({"ids": [id.0, line.0]}));
    run(&mut s, "text.attach_path", json!({}));
    let p = text_of(&s, id);
    assert!(p.on_path.is_some() && p.fills.len() == 2);
    run(&mut s, "text.release_path", json!({}));
    // Break Apart: one shape per colour.
    run(&mut s, "element.select", json!({"ids": [id.0]}));
    let shapes_before = s.visible_elements().iter().filter(|e| matches!(e.kind, ElementKind::Shape(_))).count();
    run(&mut s, "modify.break_apart", json!({}));
    let shapes = s.visible_elements().iter().filter(|e| matches!(e.kind, ElementKind::Shape(_))).count();
    assert_eq!(shapes, shapes_before + 2);
}

#[test]
fn whole_block_keys_restyle_every_run_and_hostile_runs_are_capped() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let id = hello(&mut s);
    run(&mut s, "text.style", json!({"start": 0, "end": 5, "bold": true}));
    // A size for the whole block keeps the bold run bold.
    run(&mut s, "text.edit", json!({"size": 24}));
    let t = text_of(&s, id);
    assert_eq!(t.runs.iter().map(|r| (r.style.size, r.style.bold)).collect::<Vec<_>>(), vec![(24.0, true), (24.0, false)]);
    // Thousands of one-character runs, alternating: capped at the limit, laid out.
    let text = "ab".repeat(6000);
    let runs: Vec<Value> = (0..anicraft_doc::MAX_TEXT_RUNS).map(|i| json!({"chars": 1, "bold": i % 2 == 0})).collect();
    let t0 = std::time::Instant::now();
    run(&mut s, "text.edit", json!({"text": text, "runs": runs, "width": 800}));
    eprintln!("{} runs edited in {:?}", anicraft_doc::MAX_TEXT_RUNS, t0.elapsed());
    let t = text_of(&s, id);
    assert_eq!(t.runs.len(), anicraft_doc::MAX_TEXT_RUNS);
    assert_eq!(t.styled_runs().iter().map(|r| r.chars).sum::<u32>(), 12_000);
    let too_many: Vec<Value> = (0..anicraft_doc::MAX_TEXT_RUNS + 1).map(|_| json!({"chars": 1})).collect();
    assert!(s.execute("text.edit", &json!({"runs": too_many})).is_err());
    assert!(s.execute("text.edit", &json!({"runs": [{"size": 3}]})).is_err(), "a run needs its length");
    assert!(s.execute("text.edit", &json!({"runs": [{"chars": 1, "size": 1e9}]})).is_err());
}

#[test]
fn every_export_draws_each_colour_and_svg_keeps_the_link() {
    if !fonts() {
        return;
    }
    let mut s = Session::default();
    let id = hello(&mut s);
    run(&mut s, "text.style", json!({"start": 0, "end": 5, "color": "#00aa00", "url": "https://example.org/?a=1&b=2"}));
    let dl = anicraft_doc::evaluate(&s.doc, 0);
    let svg = anicraft_render::svg::to_svg(&dl, &s.doc.stage);
    assert!(svg.contains("<a href=\"https://example.org/?a=1&amp;b=2\">"), "the linked word is a link");
    assert_eq!(svg.matches("</a>").count(), 1);
    assert!(svg.contains("#00aa00") || svg.contains("rgb(0,170,0)"), "green piece");
    let html = anicraft_render::html::to_html(&s.doc, &anicraft_render::html::HtmlOptions::default()).unwrap().html;
    assert!(html.matches("{\"s\":").count() >= 2, "the page draws a paint per colour");
    let lottie = anicraft_render::lottie::to_lottie(&s.doc, "t", 0, 0, false).json;
    assert!(lottie.matches("\"ty\":\"fl\"").count() >= 2, "a fill per colour");
    // A field's authored text keeps its colours in the page, as pieces.
    run(&mut s, "element.select", json!({"ids": [id.0]}));
    run(&mut s, "text.field", json!({"type": "dynamic"}));
    let html = anicraft_render::html::to_html(&s.doc, &anicraft_render::html::HtmlOptions::default()).unwrap().html;
    assert!(html.contains("\"pp\":"), "field pieces");
}

#[test]
fn svg_links_are_sanitized_and_round_trip_without_changing_the_drawing() {
    // Exercise the export boundary without installed fonts or text shaping.
    let mut doc = anicraft_doc::Document::default();
    doc.stage.width = 32;
    doc.stage.height = 24;
    let mut s = Session::new(doc);
    run(&mut s, "draw.rect", json!({"x": 4, "y": 4, "w": 12, "h": 8, "fill": "#336699", "stroke": null}));
    let expected = anicraft_render::render_frame(&s.doc, 0, 1.0).unwrap().to_rgba8();
    assert!(expected.as_chunks::<4>().0.iter().any(|p| p == &[0x33, 0x66, 0x99, 0xff]), "the fixture draws a coloured rectangle");
    let mut dl = anicraft_doc::evaluate(&s.doc, 0);
    assert_eq!(dl.items.len(), 1);
    for (index, (url, href)) in [
        (" \thttps://example.org/?a=1&b=\"<>'\r\n\u{7f}ok ", Some("https://example.org/?a=1&b=\"<>'ok")),
        ("java\tscript:alert(1)", None),
        ("vb\rscript:msgbox(1)", None),
        ("da\nta:text/plain,hello", None),
    ]
    .into_iter()
    .enumerate()
    {
        dl.links = vec![(0, url.into())];
        let svg = anicraft_render::svg::to_svg(&dl, &s.doc.stage);
        let root = anicraft_xml::parse(svg.as_bytes()).unwrap();
        let anchors: Vec<_> = root.children.iter().filter(|el| el.name == "a").collect();
        assert_eq!(anchors.len(), usize::from(href.is_some()), "{url:?}: {svg}");
        assert_eq!(anchors.first().and_then(|el| el.attr("href")), href, "decoded href for {url:?}");
        if let Some(anchor) = anchors.first() {
            assert!(anchor.child("path").is_some(), "the anchor wraps the artwork");
        }

        // Use the real file-import command rather than duplicating its document mapping.
        let path = std::env::temp_dir().join(format!("anicraft-svg-link-roundtrip-{}-{index}.svg", std::process::id()));
        std::fs::write(&path, svg.as_bytes()).unwrap();
        let back = anicraft_doc::Document { stage: s.doc.stage, ..Default::default() };
        let mut imported = Session::new(back);
        let result = imported.execute("file.import", &json!({"path": path.display().to_string(), "x": 0, "y": 0}));
        std::fs::remove_file(&path).unwrap();
        result.unwrap_or_else(|e| panic!("file.import for {url:?}: {e}"));
        let actual = anicraft_render::render_frame(&imported.doc, 0, 1.0).unwrap().to_rgba8();
        assert_eq!(actual, expected, "link handling must preserve rendered geometry for {url:?}");
    }
}
