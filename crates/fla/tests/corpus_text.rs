//! Differential text test against the local (uncommitted) ground-truth corpus, run only when
//! it is available: `ANICRAFT_FLA_CORPUS=/path/to/corpus cargo test -p anicraft-fla --test
//! corpus_text -- --nocapture`.
//!
//! Every `CPicText` record in the legacy files is decoded and paired with its Animate twin's
//! `DOMStaticText` / `DOMDynamicText` (by characters and position), then compared field by
//! field, element and merged run alike. The corpus files are EDAP Tools sample rigs (CC
//! BY-NC-SA), so they stay out of the MIT-licensed repo: these tests observe behaviour, they
//! don't commit fixtures.

use anicraft_fla::text::{self, Alignment, TextRecord};
use anicraft_xml::Element as X;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn corpus() -> Option<PathBuf> {
    std::env::var_os("ANICRAFT_FLA_CORPUS").map(PathBuf::from)
}

/// The corpus's legacy files and their Animate twins (a zip package, or the extracted folder
/// where the twin's zip directory is damaged).
const TWINS: &[(&str, &str)] = &[
    ("giskard-legacy.fla", "giskard-animate.fla"),
    ("kinefox-legacy.fla", "kinefox-animate.fla"),
    ("mademoiselle-legacy.fla", "mademoiselle-animate"),
    ("magnebot-legacy.fla", "magnebot-animate.fla"),
    ("movietemplate-legacy.fla", "movietemplate-animate.fla"),
    ("rabbit-legacy.fla", "rabbit-animate.fla"),
    ("robokit-legacy/EDAPT_SMR_FreeStyle_RoboKit_v1b.fla", "robokit-animate/EDAPT_SMR_FreeStyle_RoboKit_v1b_CC.fla"),
    ("storyboard-legacy/EDAPT_Storyboard_Template.fla", "storyboard-animate/EDAPT_Storyboard_Template_CC.fla"),
    ("storyboard-legacy/EDAPT_Storyboard_Template_Sample_Animatic.fla", "storyboard-animate/EDAPT_Storyboard_Template_Sample_Animatic_CC.fla"),
];

/// The one known difference: Animate recomputed this text's bounds on resave, one twip
/// narrower on each side (the legacy value is authoritative).
const TOLERATED: &[(&str, &str, usize, &str)] = &[("mademoiselle-legacy.fla", "P 2 1609991139", 3619, "left"), ("mademoiselle-legacy.fla", "P 2 1609991139", 3619, "width")];

/// A twin text element: where it was, its kind, attributes, matrix and runs.
struct Twin {
    member: String,
    kind: String,
    attrs: BTreeMap<String, String>,
    matrix: BTreeMap<String, String>,
    runs: Vec<(String, BTreeMap<String, String>)>,
}

impl Twin {
    fn text(&self) -> String {
        self.runs.iter().map(|(c, _)| c.as_str()).collect()
    }
    fn m(&self, k: &str, default: f64) -> f64 {
        num(self.matrix.get(k), default)
    }
}

fn num(v: Option<&String>, default: f64) -> f64 {
    v.and_then(|s| s.trim().parse().ok()).unwrap_or(default)
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn child<'a>(el: &'a X, name: &str) -> Option<&'a X> {
    el.children.iter().find(|c| local(&c.name) == name)
}

fn attrs(el: &X) -> BTreeMap<String, String> {
    el.attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Every text element in a twin XML, in document order (frames and group members alike).
fn walk(el: &X, member: &str, out: &mut Vec<Twin>) {
    let name = local(&el.name);
    if matches!(name, "DOMStaticText" | "DOMDynamicText" | "DOMInputText") {
        let matrix = child(el, "matrix").and_then(|m| child(m, "Matrix")).map(attrs).unwrap_or_default();
        let runs = child(el, "textRuns")
            .map(|r| {
                r.children.iter().filter(|c| local(&c.name) == "DOMTextRun").map(|run| {
                    let chars = child(run, "characters").map(|c| c.text.clone()).unwrap_or_default();
                    let a = child(run, "textAttrs").and_then(|t| child(t, "DOMTextAttrs")).map(attrs).unwrap_or_default();
                    (chars, a)
                }).collect()
            })
            .unwrap_or_default();
        out.push(Twin { member: member.to_string(), kind: name.to_string(), attrs: attrs(el), matrix, runs });
        return;
    }
    for c in &el.children {
        walk(c, member, out);
    }
}

/// The twin's documents: `DOMDocument.xml` and every library item.
fn twin_texts(path: &Path) -> Vec<Twin> {
    let mut docs: Vec<(String, Vec<u8>)> = Vec::new();
    if path.is_dir() {
        docs.push(("DOMDocument.xml".into(), std::fs::read(path.join("DOMDocument.xml")).expect("twin document reads")));
        let mut dirs = vec![path.join("LIBRARY")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("twin library lists") {
                let p = entry.expect("entry").path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|e| e == "xml") {
                    let name = p.strip_prefix(path).expect("inside the twin").to_string_lossy().replace('\\', "/");
                    docs.push((name, std::fs::read(&p).expect("twin item reads")));
                }
            }
        }
    } else {
        let bytes = std::fs::read(path).expect("twin reads");
        let archive = craft_xfl::Archive::open(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let names: Vec<String> = archive.names().filter(|n| *n == "DOMDocument.xml" || (n.starts_with("LIBRARY/") && n.ends_with(".xml"))).map(str::to_string).collect();
        for name in names {
            if let Ok(Some(xml)) = archive.read(&name) {
                docs.push((name, xml));
            }
        }
    }
    let mut out = Vec::new();
    for (name, xml) in docs {
        let root = anicraft_xml::parse(&xml).unwrap_or_else(|e| panic!("{}: {name}: {e:?}", path.display()));
        walk(&root, &name, &mut out);
    }
    out
}

/// The stem of a twin member (`LIBRARY/Copyright/CC_note.xml` → `CC_note`).
fn stem(member: &str) -> &str {
    let file = member.rsplit('/').next().unwrap_or(member);
    file.strip_suffix(".xml").unwrap_or(file)
}

/// Field-by-field comparison of a record and its twin: (check name, equal) pairs.
fn compare(t: &TextRecord, w: &Twin) -> Vec<(&'static str, bool)> {
    let near = |a: f64, b: f64| (a - b).abs() < 1e-4;
    let a = |k: &str| w.attrs.get(k).map(String::as_str);
    let m = t.matrix.m;
    let mut out = vec![
        ("matrix a b c d", [("a", 1.0), ("b", 0.0), ("c", 0.0), ("d", 1.0)].iter().zip(&m[..4]).all(|((k, d), v)| near(*v as f64, w.m(k, *d)))),
        ("matrix tx ty", near(m[4] as f64, w.m("tx", 0.0)) && near(m[5] as f64, w.m("ty", 0.0))),
        ("left", near(t.left() as f64, num(w.attrs.get("left"), 0.0))),
        ("width", near(t.width() as f64, num(w.attrs.get("width"), 0.0))),
        ("height", near(t.height() as f64, num(w.attrs.get("height"), 0.0))),
        ("autoExpand", t.auto_expand == (a("autoExpand") == Some("true"))),
        ("selected", t.selected == (a("selected") == Some("true"))),
        ("locked", t.locked == (a("locked") == Some("true"))),
        ("static or dynamic", (t.kind == anicraft_doc::TextKind::Dynamic) == (w.kind == "DOMDynamicText") && w.kind != "DOMInputText"),
        ("lineType", (t.lines == anicraft_doc::LineType::Multiline) == (a("lineType") == Some("multiline"))),
        ("static isSelectable", t.kind != anicraft_doc::TextKind::Static || t.selectable == (a("isSelectable") != Some("false"))),
        ("dynamic isSelectable", t.kind == anicraft_doc::TextKind::Static || a("isSelectable") != Some("false")),
        ("antiAliasThickness", match (t.anti_alias_thickness(), a("antiAliasThickness")) {
            (None, None) => true,
            (Some(v), Some(x)) => x.parse::<f32>().is_ok_and(|x| x == v),
            _ => false,
        }),
        ("fontRenderingMode", t.standard_rendering() == (a("fontRenderingMode") == Some("standard"))),
        ("run count", t.runs.len() == w.runs.len()),
    ];
    if t.runs.len() != w.runs.len() {
        return out;
    }
    for (r, (chars, ra)) in t.runs.iter().zip(&w.runs) {
        let s = &r.style;
        let g = |k: &str| ra.get(k).map(String::as_str);
        let face = g("face").unwrap_or("");
        let align = match s.align {
            Alignment::Left => None,
            Alignment::Right => Some("right"),
            Alignment::Center => Some("center"),
            Alignment::Justify => Some("justify"),
        };
        let fill = format!("#{:02X}{:02X}{:02X}", s.rgba[0], s.rgba[1], s.rgba[2]);
        out.extend([
            ("run characters", &r.text == chars),
            ("run size", near(s.size as f64, num(ra.get("size"), 12.0))),
            ("run face", !s.face.is_empty() && face.starts_with(s.face.as_str())),
            ("run bold", s.bold == face.contains("Bold")),
            ("run italic", s.italic == face.contains("Italic")),
            ("run fillColor", fill == g("fillColor").unwrap_or("#000000").to_ascii_uppercase()),
            ("run alpha", (s.rgba[3] as f64 / 255.0 - num(ra.get("alpha"), 1.0)).abs() < 0.003),
            ("run autoKern", s.auto_kern == (g("autoKern") != Some("false"))),
            ("run characterPosition", (s.shift == anicraft_doc::BaselineShift::Superscript) == (g("characterPosition") == Some("superscript"))),
            ("run alignment", align == g("alignment")),
            // Animate leaves lineSpacing out at its default, 2 px (stored as 40 twips).
            ("run lineSpacing", near(s.line_spacing as f64, num(ra.get("lineSpacing"), 2.0))),
            ("run indent", near(s.indent as f64, num(ra.get("indent"), 0.0))),
            ("run letterSpacing", near(s.letter_spacing as f64, num(ra.get("letterSpacing"), 0.0))),
            ("run target", s.target == g("target").unwrap_or("")),
        ]);
    }
    out
}

#[test]
fn every_corpus_text_record_decodes_like_its_twin() {
    let Some(dir) = corpus() else { return };
    let fonts = anicraft_text::Fonts::get();
    let has_fonts = fonts.resolve("").is_some();
    let (mut heads, mut decoded, mut paired, mut exact, mut elements) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut checks: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
    let mut problems: Vec<String> = Vec::new();
    let mut approximated: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let data = std::fs::read(&legacy_path).expect("legacy reads");
        let file = anicraft_fla::cfb::Compound::new(&data).expect("opens");
        let contents = file.stream("Contents").expect("Contents reads");
        let labels: BTreeMap<String, String> = anicraft_fla::doc::library(&contents, &mut anicraft_fla::Report::default()).into_iter().collect();
        let twins = twin_texts(&twin_path);
        let mut used = vec![false; twins.len()];
        let (mut faces, mut report) = (text::Faces::new(), anicraft_fla::Report::default());
        for entry in file.entries() {
            if entry.kind != anicraft_fla::cfb::Kind::Stream {
                continue;
            }
            let bytes = file.stream(&entry.path).expect("stream reads");
            let found = text::find_all(&bytes);
            heads += text::heads(&bytes).len();
            decoded += found.len();
            let label = labels.get(&entry.path).map(String::as_str);
            for t in &found {
                for what in &t.approximations {
                    *approximated.entry(what).or_default() += 1;
                }
                let chars: String = t.runs.iter().map(|r| r.text.as_str()).collect();
                let (tx, ty) = (t.matrix.m[4] as f64, t.matrix.m[5] as f64);
                let candidates: Vec<usize> = (0..twins.len())
                    .filter(|&i| !used[i] && twins[i].text() == chars && (twins[i].m("tx", 0.0) - tx).abs() < 0.01 && (twins[i].m("ty", 0.0) - ty).abs() < 0.01)
                    .collect();
                // The twin in the same library item (or the document, for a page) first; among
                // those, one that matches field for field.
                let home = |i: &usize| match label {
                    Some(l) => stem(&twins[*i].member) == l.rsplit('/').next().unwrap_or(l),
                    None => twins[*i].member == "DOMDocument.xml",
                };
                let mut order: Vec<usize> = candidates.iter().copied().filter(home).collect();
                order.extend(candidates.iter().copied().filter(|i| !home(i)));
                let Some(&first) = order.first() else {
                    problems.push(format!("{legacy} {} @{}: no twin for {:?}", entry.path, t.head, chars.chars().take(30).collect::<String>()));
                    continue;
                };
                let pick = order.iter().copied().find(|&i| compare(t, &twins[i]).iter().all(|(_, ok)| *ok)).unwrap_or(first);
                used[pick] = true;
                paired += 1;
                let mut all = true;
                for (name, ok) in compare(t, &twins[pick]) {
                    let tolerated = TOLERATED.iter().any(|(f, s, h, c)| f == legacy && *s == entry.path && *h == t.head && *c == name);
                    let e = checks.entry(name).or_default();
                    e.1 += 1;
                    if ok || tolerated {
                        e.0 += 1;
                    }
                    if !ok {
                        all = false;
                        if !tolerated {
                            problems.push(format!("{legacy} {} @{}: {name} differs from {} ({})", entry.path, t.head, twins[pick].member, twins[pick].kind));
                        }
                    }
                }
                exact += all as usize;
                // The record as a document element, laid out as the XFL importer lays out
                // the twin: its text, kind and line type.
                if has_fonts {
                    let element = t.to_element(anicraft_doc::ElementId(1), fonts, &mut faces, &mut report);
                    let shown = chars.replace(['\r'], "\n");
                    match (&element, t.kind) {
                        (Some(e), kind) => {
                            elements += 1;
                            let anicraft_doc::ElementKind::Text(b) = &e.kind else { panic!("a text element") };
                            let want = if kind == anicraft_doc::TextKind::Static { shown.trim_end() } else { shown.trim_end_matches('\n') };
                            if b.text != want || b.field.as_ref().map(|f| (f.kind, f.lines)) != (kind != anicraft_doc::TextKind::Static).then_some((kind, t.lines)) {
                                problems.push(format!("{legacy} {} @{}: element text or field differs", entry.path, t.head));
                            }
                        }
                        (None, anicraft_doc::TextKind::Static) if shown.trim_end().is_empty() => {}
                        (None, _) => problems.push(format!("{legacy} {} @{}: no element", entry.path, t.head)),
                    }
                }
            }
        }
        let unused = used.iter().filter(|u| !**u).count();
        if unused > 0 {
            problems.push(format!("{legacy}: {unused} twin texts without a legacy record"));
        }
    }
    for (name, (ok, n)) in &checks {
        eprintln!("{name:24} {ok:4}/{n:4}");
    }
    eprintln!("approximated: {approximated:?}");
    eprintln!("{decoded}/{heads} text records decoded, {paired} paired with twins, {exact} exact on every field, {elements} document elements");
    assert!(problems.is_empty(), "{} problems: {:#?}", problems.len(), &problems[..problems.len().min(30)]);
    assert_eq!(decoded, heads, "every text head decodes");
    assert_eq!(paired, 372, "the corpus's 372 texts pair with their twins");
    assert!(exact >= 371, "all but the one tolerated text match on every field, got {exact}");
}
