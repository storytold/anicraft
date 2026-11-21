//! Corpus tests, run only when the local (uncommitted) ground-truth corpus is available:
//! `ANICRAFT_FLA_CORPUS=/path/to/corpus cargo test -p anicraft-fla --test corpus`.
//!
//! The corpus files are EDAP Tools sample rigs (CC BY-NC-SA, "test/educational only, no
//! commercial"), so they stay out of the MIT-licensed repo; these tests observe behaviour,
//! they don't commit fixtures.

use std::path::PathBuf;

fn corpus() -> Option<PathBuf> {
    std::env::var_os("ANICRAFT_FLA_CORPUS").map(PathBuf::from)
}

#[test]
fn every_legacy_fla_in_the_corpus_opens_and_lists_streams() {
    let Some(dir) = corpus() else { return };
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&dir).expect("corpus directory lists") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "fla") {
            continue;
        }
        let data = std::fs::read(&path).expect("file reads");
        if !anicraft_fla::is_legacy(&data) {
            continue; // the zip-wrapped twins of the corpus are not this crate's input
        }
        let streams = anicraft_fla::dump(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!streams.is_empty(), "{}: no streams", path.display());
        let names: Vec<&str> = streams.iter().map(|s| s.path.as_str()).collect();
        assert!(names.contains(&"Contents"), "{}: no Contents stream ({names:?})", path.display());
        checked += 1;
    }
    if checked == 0 {
        panic!("the corpus directory held no legacy .fla files");
    }
}

#[test]
fn the_corpus_contents_stream_starts_with_the_document_prologue() {
    let Some(dir) = corpus() else { return };
    for entry in std::fs::read_dir(&dir).expect("corpus directory lists") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "fla") {
            continue;
        }
        let data = std::fs::read(&path).expect("file reads");
        if !anicraft_fla::is_legacy(&data) {
            continue;
        }
        let file = anicraft_fla::cfb::Compound::new(&data).expect("opens");
        let contents = file.stream("Contents").expect("Contents reads");
        assert_eq!(&contents[..5], &anicraft_fla::records::CONTENTS_PROLOGUE, "{}", path.display());
    }
}

/// Ground truth from the corpus's modern twins (the same rigs as Animate XFL): scenes
/// named "Scene 1", layers top-first in the twin's DOM order.
fn expected_layers(name: &str) -> Option<Vec<&str>> {
    Some(match name {
        "giskard-legacy.fla" => vec!["notes", "character"],
        "kinefox-legacy.fla" => vec!["notes", "character", "poses"],
        "mademoiselle-legacy.fla" => vec!["notes", "character"],
        "magnebot-legacy.fla" => vec!["notes", "character"],
        "rabbit-legacy.fla" => vec!["notes", "character", "poses"],
        // The twin has one more layer, named after the audio's sound file (the timeline
        // differential below checks every layer exactly).
        "movietemplate-legacy.fla" => {
            vec!["notes", "audio", "sound 1", "sound 2", "maracas", "video", "frame counter", "field guide / mask", "fade in / out", "FX", "camera"]
        }
        _ => return None,
    })
}

#[test]
fn the_corpus_opens_with_the_twins_scenes_and_layers() {
    let Some(dir) = corpus() else { return };
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&dir).expect("corpus directory lists") {
        let path = entry.expect("entry").path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(want_layers) = expected_layers(name) else { continue };
        let data = std::fs::read(&path).expect("file reads");
        if !anicraft_fla::is_legacy(&data) {
            continue;
        }
        let (doc, report) = anicraft_fla::import(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(doc.scenes.len(), 1, "{}: one scene", path.display());
        assert_eq!(doc.scenes[0].name, "Scene 1", "{}", path.display());
        let got: Vec<&str> = doc.scenes[0].timeline.layers.iter().map(|l| l.name.as_str()).collect();
        if name == "movietemplate-legacy.fla" {
            // These layers are all present, top-first, with the sound-named one among them.
            for real in &want_layers {
                assert!(got.contains(real), "{}: layer {real:?} missing from {got:?}", path.display());
            }
            assert!(got.len() >= want_layers.len(), "{}", path.display());
        } else {
            assert_eq!(got, want_layers, "{}: layer names top-first like the twin", path.display());
        }
        assert!(!report.skipped.is_empty(), "{}: a structure-only open reports what stayed out", path.display());
        checked += 1;
    }
    assert!(checked >= 6, "expected the corpus's 6 legacy pairs, ran {checked}");
}


#[test]
fn the_corpus_decodes_library_shapes_with_their_curves() {
    let Some(dir) = corpus() else { return };
    let (mut symbols, mut curved, mut files) = (0usize, 0usize, 0usize);
    for entry in std::fs::read_dir(&dir).expect("corpus directory lists") {
        let path = entry.expect("entry").path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with("-legacy.fla") { continue; }
        let data = std::fs::read(&path).expect("file reads");
        if !anicraft_fla::is_legacy(&data) { continue; }
        let (doc, report) = anicraft_fla::import(&data).expect("import");
        let shapes = |s: &anicraft_doc::Symbol| s.timeline.layers.iter().flat_map(|l| l.keyframes.iter()).flat_map(|k| k.elements.iter()).filter_map(|e| match &e.kind {
            anicraft_doc::ElementKind::Shape(shape) => Some(shape.path.clone()),
            _ => None,
        }).collect::<Vec<_>>();
        let with_art: Vec<_> = doc.library.iter().map(|s| shapes(s)).filter(|p| !p.is_empty()).collect();
        if !with_art.is_empty() { files += 1; }
        symbols += with_art.len();
        curved += with_art.iter().filter(|paths| paths.iter().any(|p| p.cmds.iter().any(|c| matches!(c, anicraft_geom::PathCmd::QuadTo(..))))).count();
        // Every first shape record of the corpus decodes: none is left out.
        let left_out: Vec<_> = report.skipped.keys().filter(|k| k.starts_with("shapes with") || k.starts_with("shapes (edge header") || k.starts_with("shapes whose")).collect();
        assert!(left_out.is_empty(), "{name}: {left_out:?}");
    }
    // 192 symbols (151 with curves) across the six root files when this was written.
    eprintln!("{symbols} library symbols with artwork, {curved} with curves");
    assert!(symbols >= 180, "expected the library's shapes to decode, got {symbols} symbols");
    assert!(curved >= 140, "expected curved artwork, got {curved} symbols with curves");
    assert!(files >= 6, "expected every corpus file to show artwork, got {files}");
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

/// The twin's library items by name (the file stem, as the legacy `Contents` labels them),
/// each with its XML text.
fn twin_library(path: &std::path::Path) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut add = |name: &str, xml: String| {
        if let Some(stem) = name.strip_suffix(".xml").map(|n| n.rsplit('/').next().unwrap_or(n)) {
            out.entry(stem.to_string()).or_default().push(xml);
        }
    };
    if path.is_dir() {
        let mut dirs = vec![path.join("LIBRARY")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("twin library lists") {
                let p = entry.expect("entry").path();
                if p.is_dir() {
                    dirs.push(p);
                } else if let Ok(xml) = std::fs::read_to_string(&p) {
                    add(&p.to_string_lossy().replace('\\', "/"), xml);
                }
            }
        }
    } else {
        let bytes = std::fs::read(path).expect("twin reads");
        let archive = craft_xfl::Archive::open(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let names: Vec<String> = archive.names().filter(|n| n.starts_with("LIBRARY/") && n.ends_with(".xml")).map(str::to_string).collect();
        for name in names {
            if let Ok(Some(xml)) = archive.read(&name) {
                add(&name, String::from_utf8_lossy(&xml).into_owned());
            }
        }
    }
    out
}

/// One twin edge segment in pixels with its styles (stroke, left fill, right fill).
#[derive(Clone, Copy, Debug)]
struct TwinSeg {
    from: (f32, f32),
    ctrl: Option<(f32, f32)>,
    to: (f32, f32),
    styles: (u32, u32, u32),
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    tag.get(at..).and_then(|rest| rest.find('"').and_then(|end| rest.get(..end)))
}

/// Every `DOMShape` in a twin XML, as its `<Edge>` segments (the `edges` attribute; the
/// `cubics` drawing hints are not edges).
fn twin_shapes(xml: &str) -> Vec<Vec<TwinSeg>> {
    let mut out = Vec::new();
    for shape in xml.split("<DOMShape").skip(1) {
        let shape = shape.split("</DOMShape>").next().unwrap_or("");
        let mut segs = Vec::new();
        for edge in shape.split("<Edge ").skip(1) {
            let tag = format!(" {}", edge.split('>').next().unwrap_or(""));
            let tag = tag.as_str();
            let Some(edges) = attr(tag, "edges") else { continue };
            let edges = edges.replace("&#xD;", " ").replace("&#xA;", " ").replace("&#13;", " ").replace("&#10;", " ");
            let n = |k| attr(tag, k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            let styles = (n("strokeStyle"), n("fillStyle0"), n("fillStyle1"));
            for s in craft_xfl::edges::parse_edge_segments(&edges) {
                segs.push(TwinSeg { from: (s.from.x, s.from.y), ctrl: s.control.map(|c| (c.x, c.y)), to: (s.to.x, s.to.y), styles });
            }
        }
        out.push(segs);
    }
    out
}

/// Within half a pixel (the requirement; the decodes agree far closer).
fn near(a: (f32, f32), b: anicraft_geom::Vec2) -> bool {
    (a.0 - b.x).abs() <= 0.5 && (a.1 - b.y).abs() <= 0.5
}

/// Whether a decoded record and a twin shape hold the same edges: every segment's end
/// points and control point within 0.5 px, its styles equal (a segment the twin stores
/// reversed has its left and right fills swapped), one to one.
fn same_edges(record: &anicraft_fla::shape::ShapeRecord, twin: &[TwinSeg]) -> bool {
    if record.edges.len() != twin.len() {
        return false;
    }
    let cell = |p: (f32, f32)| (p.0.floor() as i64, p.1.floor() as i64);
    let mut by_to: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for (i, t) in twin.iter().enumerate() {
        by_to.entry(cell(t.to)).or_default().push(i);
    }
    let mut used = vec![false; twin.len()];
    for e in &record.edges {
        let s = e.seg;
        let styles = (e.stroke as u32, e.fill_left as u32, e.fill_right as u32);
        // The nearest unused candidate, as drawn or reversed (tiny segments sit within
        // half a pixel of several others, so the first that fits isn't always the one).
        let d = |a: (f32, f32), b: anicraft_geom::Vec2| (a.0 - b.x).abs() + (a.1 - b.y).abs();
        let ctrl = |t: &TwinSeg| match (t.ctrl, s.ctrl) {
            (Some(a), Some(b)) => near(a, b).then(|| d(a, b)),
            (None, None) => Some(0.0),
            _ => None,
        };
        let forward = |t: &TwinSeg| (near(t.to, s.to) && near(t.from, s.from) && t.styles == styles).then(|| ctrl(t).map(|c| c + d(t.to, s.to) + d(t.from, s.from))).flatten();
        let reverse = |t: &TwinSeg| (near(t.to, s.from) && near(t.from, s.to) && t.styles == (styles.0, styles.2, styles.1)).then(|| ctrl(t).map(|c| c + d(t.to, s.from) + d(t.from, s.to))).flatten();
        let mut found: Option<(f32, usize)> = None;
        for (target, test) in [(s.to, &forward as &dyn Fn(&TwinSeg) -> Option<f32>), (s.from, &reverse)] {
            let (cx, cy) = cell((target.x, target.y));
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &i in by_to.get(&(cx + dx, cy + dy)).map(Vec::as_slice).unwrap_or(&[]) {
                        if let (false, Some(cost)) = (used[i], test(&twin[i]))
                            && found.is_none_or(|(best, _)| cost < best)
                        {
                            found = Some((cost, i));
                        }
                    }
                }
            }
        }
        let found = found.map(|(_, i)| i);
        match found {
            Some(i) => used[i] = true,
            None => return false,
        }
    }
    true
}

#[test]
fn the_corpus_decodes_curved_shapes_edge_for_edge_like_the_twins() {
    let Some(dir) = corpus() else { return };
    let (mut matched, mut curved, mut ufo) = (0usize, 0usize, false);
    let mut mismatched = Vec::new();
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let data = std::fs::read(&legacy_path).expect("legacy reads");
        let file = anicraft_fla::cfb::Compound::new(&data).expect("opens");
        let contents = file.stream("Contents").expect("Contents reads");
        let library = anicraft_fla::doc::library(&contents, &mut anicraft_fla::Report::default());
        let twins = twin_library(&twin_path);
        for (stream, label) in &library {
            let (Ok(bytes), Some(xmls)) = (file.stream(stream), twins.get(label)) else { continue };
            let candidates: Vec<Vec<TwinSeg>> = xmls.iter().flat_map(|x| twin_shapes(x)).collect();
            for record in anicraft_fla::shape::find_all(&bytes).iter().filter(|r| !r.edges.is_empty()) {
                if candidates.iter().any(|t| same_edges(record, t)) {
                    matched += 1;
                    let curves = record.edges.iter().filter(|e| e.seg.ctrl.is_some()).count();
                    if curves > 0 {
                        curved += 1;
                    }
                    // The handoff's fixture: the UFO's lights, 11 loops of 8 quadratics.
                    if *legacy == "movietemplate-legacy.fla" && label == "UFO" && record.edges.len() == 88 && curves == 88 {
                        ufo = true;
                    }
                } else {
                    mismatched.push(format!("{legacy} {stream} ({label}): {} edges", record.edges.len()));
                }
            }
        }
    }
    assert!(mismatched.is_empty(), "{} decoded shapes differ from every twin shape: {:?}", mismatched.len(), &mismatched[..mismatched.len().min(20)]);
    assert!(ufo, "the UFO's 88 quadratic edges match its twin");
    assert!(matched >= 3000, "expected the corpus's ~3200 shapes to match their twins, matched {matched}");
    assert!(curved >= 1500, "expected curved shapes among them, got {curved}");
    eprintln!("{matched} decoded shapes match their twins edge for edge ({curved} with curves)");
}

/// The XML of a twin's member: from its zip package, or its extracted folder.
fn twin_member(path: &std::path::Path, member: &str) -> Option<Vec<u8>> {
    if path.is_dir() {
        return std::fs::read(path.join(member)).ok();
    }
    let bytes = std::fs::read(path).ok()?;
    let archive = craft_xfl::Archive::open(&bytes).ok()?;
    archive.read(member).ok().flatten()
}

fn twin_members(path: &std::path::Path) -> Vec<String> {
    if path.is_dir() {
        let mut out = Vec::new();
        let mut dirs = vec![path.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("twin lists").flatten() {
                let p = entry.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if let Ok(rel) = p.strip_prefix(path) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        return out;
    }
    let bytes = std::fs::read(path).expect("twin reads");
    let archive = craft_xfl::Archive::open(&bytes).expect("twin opens");
    archive.names().map(str::to_string).collect()
}

/// One keyframe as compared: its span and its symbol instances (library name, matrix).
#[derive(Debug, Clone, PartialEq)]
struct KeyCmp {
    start: u32,
    duration: u32,
    /// "motion" (classic) or "shape".
    tween: Option<&'static str>,
    instances: Vec<(String, [f32; 6])>,
    /// Each instance's loop mode and first frame.
    playback: Vec<(anicraft_doc::LoopMode, u32)>,
}

#[derive(Debug, Clone)]
struct LayerCmp {
    name: String,
    keys: Vec<KeyCmp>,
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn kids<'a>(el: &'a craft_xml::El, name: &'a str) -> impl Iterator<Item = &'a craft_xml::El> + 'a {
    el.elements().filter(move |e| local(&e.name) == name)
}

/// A twin `DOMTimeline` as compared: layers top-first, folders without frames.
fn twin_timeline(tl: &craft_xml::El) -> Vec<LayerCmp> {
    let mut out = Vec::new();
    for layers in kids(tl, "layers") {
        for l in kids(layers, "DOMLayer") {
            let folder = l.attr("layerType") == Some("folder");
            let mut keys = Vec::new();
            for frames in kids(l, "frames").filter(|_| !folder) {
                for f in kids(frames, "DOMFrame") {
                    let start = f.attr("index").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let duration = f.attr("duration").and_then(|v| v.parse().ok()).unwrap_or(1);
                    let tween = match f.attr("tweenType") {
                        Some("motion") => Some("motion"),
                        Some("shape") => Some("shape"),
                        _ => None,
                    };
                    let mut instances = Vec::new();
                    let mut playback = Vec::new();
                    for els in kids(f, "elements") {
                        for e in kids(els, "DOMSymbolInstance") {
                            let name = e.attr("libraryItemName").unwrap_or("");
                            let name = name.rsplit('/').next().unwrap_or(name).to_string();
                            let m = kids(e, "matrix").flat_map(|m| kids(m, "Matrix")).next();
                            let g = |k: &str, d: f32| m.and_then(|m| m.attr(k)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
                            instances.push((name, [g("a", 1.0), g("b", 0.0), g("c", 0.0), g("d", 1.0), g("tx", 0.0), g("ty", 0.0)]));
                            let lm = match e.attr("loop") {
                                Some("play once" | "play once reverse") => anicraft_doc::LoopMode::PlayOnce,
                                Some("single frame") => anicraft_doc::LoopMode::SingleFrame,
                                _ => anicraft_doc::LoopMode::Loop,
                            };
                            playback.push((lm, e.attr("firstFrame").and_then(|v| v.parse().ok()).unwrap_or(0)));
                        }
                    }
                    keys.push(KeyCmp { start, duration, tween, instances, playback });
                }
            }
            out.push(LayerCmp { name: l.attr("name").unwrap_or("").to_string(), keys });
        }
    }
    out
}

/// An imported timeline as compared.
fn legacy_timeline(doc: &anicraft_doc::Document, tl: &anicraft_doc::Timeline) -> Vec<LayerCmp> {
    tl.layers.iter().map(|l| LayerCmp {
        name: l.name.clone(),
        keys: l.keyframes.iter().map(|k| KeyCmp {
            start: k.start,
            duration: k.duration,
            tween: k.tween.as_ref().map(|t| if t.kind == anicraft_doc::TweenKind::Shape { "shape" } else { "motion" }),
            instances: k.elements.iter().filter_map(|e| match &e.kind {
                anicraft_doc::ElementKind::Symbol { symbol, .. } => Some((doc.symbol(*symbol).map_or(String::new(), |s| s.name.clone()), e.transform.m)),
                _ => None,
            }).collect(),
            playback: k.elements.iter().filter_map(|e| match &e.kind {
                anicraft_doc::ElementKind::Symbol { loop_mode, first_frame, .. } => Some((*loop_mode, *first_frame)),
                _ => None,
            }).collect(),
        }).collect(),
    }).collect()
}

#[derive(Default, Debug)]
struct Tally {
    timelines: usize,
    timelines_same_layers: usize,
    layers: usize,
    layers_same_keys: usize,
    keyframes: usize,
    instances: usize,
    instances_same: usize,
    tweens_same: usize,
    playback_same: usize,
    mismatches: Vec<String>,
}

fn compare(tag: &str, legacy: &[LayerCmp], twin: &[LayerCmp], t: &mut Tally) {
    t.timelines += 1;
    let names = |ls: &[LayerCmp]| ls.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    if names(legacy) != names(twin) {
        t.mismatches.push(format!("{tag}: layers {:?} vs twin {:?}", names(legacy), names(twin)));
        return;
    }
    t.timelines_same_layers += 1;
    for (a, b) in legacy.iter().zip(twin) {
        t.layers += 1;
        let spans = |l: &LayerCmp| l.keys.iter().map(|k| (k.start, k.duration)).collect::<Vec<_>>();
        if spans(a) != spans(b) {
            t.mismatches.push(format!("{tag} / {}: keyframes {:?} vs twin {:?}", a.name, spans(a), spans(b)));
            continue;
        }
        t.layers_same_keys += 1;
        for (ka, kb) in a.keys.iter().zip(&b.keys) {
            t.keyframes += 1;
            if ka.tween == kb.tween {
                t.tweens_same += 1;
            } else if t.mismatches.len() < 400 {
                t.mismatches.push(format!("{tag} / {} @{}: tween {:?} vs twin {:?}", a.name, ka.start, ka.tween, kb.tween));
            }
            if ka.playback.len() == kb.playback.len() {
                for (pa, pb) in ka.playback.iter().zip(&kb.playback) {
                    if pa == pb {
                        t.playback_same += 1;
                    } else if t.mismatches.len() < 400 {
                        t.mismatches.push(format!("{tag} / {} @{}: loop/first frame {pa:?} vs twin {pb:?}", a.name, ka.start));
                    }
                }
            }
            t.instances += kb.instances.len().max(ka.instances.len());
            if ka.instances.len() != kb.instances.len() {
                t.mismatches.push(format!("{tag} / {} @{}: {} instances vs twin {}", a.name, ka.start, ka.instances.len(), kb.instances.len()));
                continue;
            }
            for ((na, ma), (nb, mb)) in ka.instances.iter().zip(&kb.instances) {
                let close = (0..4).all(|i| (ma[i] - mb[i]).abs() <= 0.01) && (4..6).all(|i| (ma[i] - mb[i]).abs() <= 0.05);
                if na == nb && close {
                    t.instances_same += 1;
                } else if t.mismatches.len() < 400 {
                    t.mismatches.push(format!("{tag} / {} @{}: {na} {ma:?} vs twin {nb} {mb:?}", a.name, ka.start));
                }
            }
        }
    }
}

#[test]
fn the_corpus_timelines_match_the_twins_keyframe_for_keyframe() {
    let Some(dir) = corpus() else { return };
    let mut all = Tally::default();
    let mut lines = Vec::new();
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let data = std::fs::read(&legacy_path).expect("legacy reads");
        let (doc, _report) = anicraft_fla::import(&data).unwrap_or_else(|e| panic!("{legacy}: {e}"));
        let mut t = Tally::default();
        // The scenes, in order.
        let dom = twin_member(&twin_path, "DOMDocument.xml").expect("DOMDocument.xml");
        let dom = craft_xfl::document::Document::parse(&dom).expect("twin parses");
        let scenes: Vec<&craft_xml::El> = dom.root().descendants().filter(|e| local(&e.name) == "DOMTimeline").collect();
        for (scene, tl) in doc.scenes.iter().zip(scenes) {
            compare(&format!("{legacy}:{}", scene.name), &legacy_timeline(&doc, &scene.timeline), &twin_timeline(tl), &mut t);
        }
        // Library symbols with a unique name in the twin.
        let mut by_stem: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for m in twin_members(&twin_path).into_iter().filter(|m| m.starts_with("LIBRARY/") && m.ends_with(".xml")) {
            let stem = m.rsplit('/').next().unwrap_or(&m).trim_end_matches(".xml").to_string();
            by_stem.entry(stem).or_default().push(m);
        }
        for symbol in &doc.library {
            let Some([member]) = by_stem.get(&symbol.name).map(Vec::as_slice) else { continue };
            let Some(xml) = twin_member(&twin_path, member) else { continue };
            let Ok(x) = craft_xfl::document::Document::parse(&xml) else { continue };
            let Some(tl) = x.root().descendants().find(|e| local(&e.name) == "DOMTimeline") else { continue };
            compare(&format!("{legacy}:{}", symbol.name), &legacy_timeline(&doc, &symbol.timeline), &twin_timeline(tl), &mut t);
        }
        lines.push(format!(
            "{legacy}: timelines {}/{} same layers, layers {}/{} same keyframes, tweens {}/{} keyframes, instances {}/{} same name and matrix, {}/{} same loop and first frame",
            t.timelines_same_layers, t.timelines, t.layers_same_keys, t.layers, t.tweens_same, t.keyframes, t.instances_same, t.instances, t.playback_same, t.instances
        ));
        all.tweens_same += t.tweens_same;
        all.playback_same += t.playback_same;
        all.timelines += t.timelines;
        all.timelines_same_layers += t.timelines_same_layers;
        all.layers += t.layers;
        all.layers_same_keys += t.layers_same_keys;
        all.keyframes += t.keyframes;
        all.instances += t.instances;
        all.instances_same += t.instances_same;
        all.mismatches.extend(t.mismatches);
    }
    for l in &lines {
        eprintln!("{l}");
    }
    eprintln!(
        "all: timelines {}/{}, layers {}/{}, tweens {}/{} keyframes, instances {}/{} (loop and first frame {}/{})",
        all.timelines_same_layers, all.timelines, all.layers_same_keys, all.layers, all.tweens_same, all.keyframes, all.instances_same, all.instances, all.playback_same, all.instances
    );
    for m in all.mismatches.iter().take(40) {
        eprintln!("  {m}");
    }
    assert!(all.timelines >= 300, "expected the corpus's ~330 timelines, compared {}", all.timelines);
    assert_eq!(all.timelines_same_layers, all.timelines, "every timeline has the twin's layers");
    assert_eq!(all.layers_same_keys, all.layers, "every layer has the twin's keyframes");
    assert_eq!(all.instances_same, all.instances, "every instance has the twin's symbol and matrix");
    assert_eq!(all.tweens_same, all.keyframes, "every keyframe has the twin's tween kind");
    assert_eq!(all.playback_same, all.instances, "every instance has the twin's loop mode and first frame");
}

#[test]
fn the_corpus_stage_matches_the_twins() {
    let Some(dir) = corpus() else { return };
    let mut checked = 0;
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let (doc, _) = anicraft_fla::import(&std::fs::read(&legacy_path).expect("reads")).expect("imports");
        let dom = twin_member(&twin_path, "DOMDocument.xml").expect("DOMDocument.xml");
        let dom = craft_xfl::document::Document::parse(&dom).expect("parses");
        let r = dom.root();
        let num = |k: &str, d: f32| r.attr(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        assert_eq!((doc.stage.width, doc.stage.height), (num("width", 550.0) as u32, num("height", 400.0) as u32), "{legacy}");
        assert!((doc.stage.fps - num("frameRate", 24.0)).abs() < 0.01, "{legacy}: {}", doc.stage.fps);
        if let Some(bg) = r.attr("backgroundColor") {
            let want = u32::from_str_radix(bg.trim_start_matches('#'), 16).expect("hex");
            let got = ((doc.stage.background.r * 255.0).round() as u32) << 16 | ((doc.stage.background.g * 255.0).round() as u32) << 8 | (doc.stage.background.b * 255.0).round() as u32;
            assert_eq!(got, want, "{legacy}: background");
        }
        checked += 1;
    }
    assert!(checked >= 6);
}

// ---------------------------------------------------------------------------------------------
// Texts, bitmaps, sounds, buttons and the library's folders, keyframe by keyframe.

/// Calls `f` for every keyframe the legacy import and its twin hold alike (same timeline,
/// layer and span: the timeline differential above checks that they all do), with a tag for
/// messages, the document, the legacy keyframe and the twin's `DOMFrame`.
fn for_each_keyframe(dir: &std::path::Path, mut f: impl FnMut(&str, &anicraft_doc::Document, &anicraft_doc::Keyframe, &craft_xml::El)) -> usize {
    let mut files = 0;
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        files += 1;
        let data = std::fs::read(&legacy_path).expect("legacy reads");
        let (doc, _) = anicraft_fla::import(&data).unwrap_or_else(|e| panic!("{legacy}: {e}"));
        let mut pairs: Vec<(String, &anicraft_doc::Timeline, Vec<u8>)> = Vec::new();
        let dom = twin_member(&twin_path, "DOMDocument.xml").expect("DOMDocument.xml");
        for (i, scene) in doc.scenes.iter().enumerate() {
            pairs.push((format!("{legacy}:{}#{i}", scene.name), &scene.timeline, dom.clone()));
        }
        let mut by_stem: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for m in twin_members(&twin_path).into_iter().filter(|m| m.starts_with("LIBRARY/") && m.ends_with(".xml")) {
            let stem = m.rsplit('/').next().unwrap_or(&m).trim_end_matches(".xml").to_string();
            by_stem.entry(stem).or_default().push(m);
        }
        for symbol in &doc.library {
            let Some([member]) = by_stem.get(&symbol.name).map(Vec::as_slice) else { continue };
            let Some(xml) = twin_member(&twin_path, member) else { continue };
            pairs.push((format!("{legacy}:{}", symbol.name), &symbol.timeline, xml));
        }
        for (tag, tl, xml) in pairs {
            let Ok(x) = craft_xfl::document::Document::parse(&xml) else { continue };
            // A scene is its DOMTimeline by position; a symbol has one.
            let scene_index = tag.rsplit_once('#').and_then(|(_, i)| i.parse::<usize>().ok());
            let mut timelines = x.root().descendants().filter(|e| local(&e.name) == "DOMTimeline");
            let Some(twin_tl) = (match scene_index {
                Some(i) => timelines.nth(i),
                None => timelines.next(),
            }) else {
                continue;
            };
            let twin_layers: Vec<&craft_xml::El> = kids(twin_tl, "layers").flat_map(|l| kids(l, "DOMLayer")).collect();
            if twin_layers.len() != tl.layers.len() {
                continue;
            }
            for (layer, tw) in tl.layers.iter().zip(twin_layers) {
                if tw.attr("layerType") == Some("folder") || tw.attr("name").unwrap_or("") != layer.name {
                    continue;
                }
                let frames: Vec<&craft_xml::El> = kids(tw, "frames").flat_map(|f| kids(f, "DOMFrame")).collect();
                if frames.len() != layer.keyframes.len() {
                    continue;
                }
                for (k, fr) in layer.keyframes.iter().zip(frames) {
                    let start: u32 = fr.attr("index").and_then(|v| v.parse().ok()).unwrap_or(0);
                    if start == k.start {
                        f(&format!("{tag} / {} @{}", layer.name, k.start), &doc, k, fr);
                    }
                }
            }
        }
    }
    files
}

/// The legacy elements of a keyframe, groups opened, each with its transform on the frame.
fn flat_elements(els: &[std::sync::Arc<anicraft_doc::Element>], parent: anicraft_geom::Affine, out: &mut Vec<(anicraft_geom::Affine, std::sync::Arc<anicraft_doc::Element>)>) {
    for e in els {
        let m = parent.mul(&e.transform);
        if let anicraft_doc::ElementKind::Group { members } = &e.kind {
            for l in &members.layers {
                for k in &l.keyframes {
                    flat_elements(&k.elements, m, out);
                }
            }
        } else {
            out.push((m, e.clone()));
        }
    }
}

/// A twin frame's elements, groups opened (Animate writes a group's members in its parent's
/// space).
fn twin_elements<'a>(el: &'a craft_xml::El, out: &mut Vec<&'a craft_xml::El>) {
    for e in el.elements() {
        match local(&e.name) {
            "DOMGroup" => {
                for m in kids(e, "members") {
                    twin_elements(m, out);
                }
            }
            _ => out.push(e),
        }
    }
}

fn twin_frame_elements(fr: &craft_xml::El) -> Vec<&craft_xml::El> {
    let mut out = Vec::new();
    for els in kids(fr, "elements") {
        twin_elements(els, &mut out);
    }
    out
}

fn twin_matrix(e: &craft_xml::El) -> anicraft_geom::Affine {
    let m = kids(e, "matrix").flat_map(|m| kids(m, "Matrix")).next();
    let g = |k: &str, d: f32| m.and_then(|m| m.attr(k)).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
    anicraft_geom::Affine { m: [g("a", 1.0), g("b", 0.0), g("c", 0.0), g("d", 1.0), g("tx", 0.0), g("ty", 0.0)] }
}

fn same_affine(a: &anicraft_geom::Affine, b: &anicraft_geom::Affine) -> bool {
    (0..4).all(|i| (a.m[i] - b.m[i]).abs() <= 0.01) && (4..6).all(|i| (a.m[i] - b.m[i]).abs() <= 0.05)
}

/// Line breaks as one character, the ends trimmed.
fn norm_text(s: &str) -> String {
    s.replace("\r\n", "\n").replace(['\r', '\u{2028}', '\u{2029}'], "\n").trim().to_string()
}

#[test]
fn the_corpus_texts_sit_on_the_twins_keyframes() {
    let Some(dir) = corpus() else { return };
    let (mut twin_total, mut matched, mut legacy_total) = (0usize, 0usize, 0usize);
    let mut mismatches = Vec::new();
    for_each_keyframe(&dir, |tag, _doc, k, fr| {
        let mut flat = Vec::new();
        flat_elements(&k.elements, anicraft_geom::Affine::IDENTITY, &mut flat);
        // Each text as (dynamic, its text, where its box's top-left (inside the gutter, for
        // static text) lands on the frame).
        let mut legacy: Vec<(bool, String, anicraft_geom::Vec2)> = flat.iter().filter_map(|(m, e)| match &e.kind {
            anicraft_doc::ElementKind::Text(b) => Some((b.field.is_some(), norm_text(&b.text), m.apply(anicraft_geom::Vec2::new(0.0, 0.0)))),
            _ => None,
        }).collect();
        legacy_total += legacy.len();
        for e in twin_frame_elements(fr) {
            let dynamic = match local(&e.name) {
                "DOMStaticText" => false,
                "DOMDynamicText" | "DOMInputText" => true,
                _ => continue,
            };
            let text: String = kids(e, "textRuns").flat_map(|r| kids(r, "DOMTextRun")).filter_map(|r| r.child_text("characters").map(|c| c.into_owned())).collect();
            let text = norm_text(&text);
            // An empty static text draws nothing: AniCraft leaves it out, as the XFL importer.
            if !dynamic && text.is_empty() {
                continue;
            }
            twin_total += 1;
            let num = |k: &str| e.attr(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            let gutter = if dynamic { 0.0 } else { anicraft_fla::text::GUTTER };
            let at = twin_matrix(e).apply(anicraft_geom::Vec2::new(num("left") + gutter, num("top") + gutter));
            let found = legacy.iter().position(|(d, t, p)| *d == dynamic && *t == text && (p.x - at.x).abs() <= 0.06 && (p.y - at.y).abs() <= 0.06);
            match found {
                Some(i) => {
                    legacy.swap_remove(i);
                    matched += 1;
                }
                None => mismatches.push(format!("{tag}: twin {} text {:?} at {at:?} not among {:?}", if dynamic { "dynamic" } else { "static" }, text.chars().take(40).collect::<String>(), legacy.iter().map(|(d, t, p)| (*d, t.chars().take(20).collect::<String>(), *p)).collect::<Vec<_>>())),
            }
        }
        for (d, t, p) in legacy {
            mismatches.push(format!("{tag}: legacy {} text {:?} at {p:?} has no twin", if d { "dynamic" } else { "static" }, t.chars().take(40).collect::<String>()));
        }
    });
    eprintln!("texts on keyframes: {matched}/{twin_total} twin texts matched ({legacy_total} legacy)");
    for m in mismatches.iter().take(30) {
        eprintln!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} text placements differ from the twins", mismatches.len());
    assert!(twin_total >= 300, "expected the corpus's texts on keyframes, compared {twin_total}");
    assert_eq!(matched, twin_total);
}

#[test]
fn the_corpus_bitmaps_and_their_instances_match_the_twins() {
    let Some(dir) = corpus() else { return };
    let (mut twin_total, mut matched) = (0usize, 0usize);
    let mut mismatches = Vec::new();
    // Every placed bitmap: the twin's library name (leaf) and matrix.
    for_each_keyframe(&dir, |tag, doc, k, fr| {
        let mut flat = Vec::new();
        flat_elements(&k.elements, anicraft_geom::Affine::IDENTITY, &mut flat);
        let mut legacy: Vec<(String, anicraft_geom::Affine)> = flat.iter().filter_map(|(m, e)| match &e.kind {
            anicraft_doc::ElementKind::Shape(s) => match s.fill {
                Some(anicraft_doc::Paint::Bitmap { bitmap, .. }) => Some((doc.bitmap(bitmap).map_or(String::new(), |b| b.name.clone()), *m)),
                _ => None,
            },
            _ => None,
        }).collect();
        for e in twin_frame_elements(fr).into_iter().filter(|e| local(&e.name) == "DOMBitmapInstance") {
            twin_total += 1;
            let name = e.attr("libraryItemName").unwrap_or("");
            let name = name.rsplit('/').next().unwrap_or(name).to_string();
            let m = twin_matrix(e);
            match legacy.iter().position(|(n, lm)| *n == name && same_affine(lm, &m)) {
                Some(i) => {
                    legacy.swap_remove(i);
                    matched += 1;
                }
                None => mismatches.push(format!("{tag}: twin bitmap {name} {m:?} not among {legacy:?}")),
            }
        }
        for l in legacy {
            mismatches.push(format!("{tag}: legacy bitmap {l:?} has no twin"));
        }
    });
    // The library's bitmaps: each twin item by its path, with its pixel size.
    let mut items = 0;
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let (doc, _) = anicraft_fla::import(&std::fs::read(&legacy_path).expect("reads")).expect("imports");
        let dom = twin_member(&twin_path, "DOMDocument.xml").expect("DOMDocument.xml");
        let dom = craft_xfl::document::Document::parse(&dom).expect("parses");
        for item in dom.root().descendants().filter(|e| local(&e.name) == "DOMBitmapItem") {
            items += 1;
            let path = item.attr("name").unwrap_or("");
            let size = |k: &str| item.attr(k).and_then(|v| v.parse::<f32>().ok()).map_or(0, |t| (t / 20.0).round() as u32);
            let found = doc.bitmaps.iter().find(|b| doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Bitmap(b.id))) == path);
            match found {
                Some(b) if (b.width, b.height) == (size("frameRight"), size("frameBottom")) => {}
                other => mismatches.push(format!("{legacy}: twin bitmap item {path} ({}×{}) vs {:?}", size("frameRight"), size("frameBottom"), other.map(|b| (b.name.clone(), b.width, b.height)))),
            }
        }
    }
    eprintln!("bitmap instances: {matched}/{twin_total} match the twins; {items} bitmap items compared");
    for m in mismatches.iter().take(30) {
        eprintln!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} bitmap differences", mismatches.len());
    assert!(twin_total >= 9 && items >= 9, "the corpus's CC badge, once per file");
}

#[test]
fn the_corpus_frame_sounds_match_the_twins() {
    let Some(dir) = corpus() else { return };
    let (mut twin_total, mut matched, mut legacy_total) = (0usize, 0usize, 0usize);
    let mut mismatches = Vec::new();
    for_each_keyframe(&dir, |tag, doc, k, fr| {
        let legacy = k.sound.as_ref();
        legacy_total += usize::from(legacy.is_some());
        let Some(name) = fr.attr("soundName").filter(|n| !n.trim().is_empty()) else {
            if let Some(s) = legacy {
                mismatches.push(format!("{tag}: legacy sound {s:?} has no twin"));
            }
            return;
        };
        twin_total += 1;
        let Some(s) = legacy else {
            mismatches.push(format!("{tag}: twin sound {name} missing"));
            return;
        };
        let leaf = name.rsplit('/').next().unwrap_or(name);
        let sync = match fr.attr("soundSync").unwrap_or("event") {
            "start" => anicraft_doc::SoundSync::Start,
            "stop" => anicraft_doc::SoundSync::Stop,
            "stream" => anicraft_doc::SoundSync::Stream,
            _ => anicraft_doc::SoundSync::Event,
        };
        let repeat: u32 = fr.attr("soundLoop").and_then(|v| v.parse().ok()).unwrap_or(1);
        // The envelope as the XFL importer reads it: its points, seconds and levels 0..=1.
        let points: Vec<(f64, f32, f32)> = kids(fr, "SoundEnvelope").flat_map(|e| kids(e, "SoundEnvelopePoint")).map(|p| {
            let n = |k: &str| p.attr(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            (n("mark44") / 44_100.0, (n("level0") / 32_768.0) as f32, (n("level1") / 32_768.0) as f32)
        }).collect();
        let got_points: Vec<(f64, f32, f32)> = match &s.effect {
            anicraft_doc::SoundEffect::Custom(p) => p.iter().map(|p| (p.at, p.left, p.right)).collect(),
            _ => Vec::new(),
        };
        let same_points = points.len() == got_points.len() && points.iter().zip(&got_points).all(|(a, b)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6 && (a.2 - b.2).abs() < 1e-6);
        let got_name = doc.sound(s.sound).map(|x| x.name.as_str()).unwrap_or("");
        if got_name == leaf && s.sync == sync && s.repeat == repeat && !s.looping && same_points {
            matched += 1;
        } else {
            mismatches.push(format!("{tag}: {got_name:?} {:?} ×{} {got_points:?} vs twin {leaf:?} {sync:?} ×{repeat} {points:?}", s.sync, s.repeat));
        }
    });
    // The library's sounds: each twin item by its path, and its length in samples.
    let mut items = 0;
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let (doc, _) = anicraft_fla::import(&std::fs::read(&legacy_path).expect("reads")).expect("imports");
        let dom = twin_member(&twin_path, "DOMDocument.xml").expect("DOMDocument.xml");
        let dom = craft_xfl::document::Document::parse(&dom).expect("parses");
        for item in dom.root().descendants().filter(|e| local(&e.name) == "DOMSoundItem") {
            items += 1;
            let path = item.attr("name").unwrap_or("");
            let found = doc.sounds.iter().find(|s| doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Sound(s.id))) == path);
            let Some(s) = found else {
                mismatches.push(format!("{legacy}: twin sound item {path} missing"));
                continue;
            };
            let count: u64 = item.attr("sampleCount").and_then(|v| v.parse().ok()).unwrap_or(0);
            let format = item.attr("format").unwrap_or("");
            let rate = if format.starts_with("44") { 44_100 } else { 0 };
            let channels = if format.ends_with("Stereo") { 2 } else { 1 };
            if (s.rate, s.channels, s.frames) != (rate, channels, count) {
                mismatches.push(format!("{legacy}: sound {path}: {} Hz × {} ch × {} vs twin {format} × {count}", s.rate, s.channels, s.frames));
            }
        }
    }
    eprintln!("frame sounds: {matched}/{twin_total} match the twins ({legacy_total} legacy); {items} sound items compared");
    for m in mismatches.iter().take(30) {
        eprintln!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} sound differences", mismatches.len());
    assert!(twin_total >= 5 && items >= 4, "the corpus's 5 sound frames and 4 sounds");
}

#[test]
fn the_corpus_instance_names_and_buttons_match_the_twins() {
    let Some(dir) = corpus() else { return };
    let (mut instances, mut named, mut buttons, mut same) = (0usize, 0usize, 0usize, 0usize);
    let mut mismatches = Vec::new();
    for_each_keyframe(&dir, |tag, doc, k, fr| {
        let legacy: Vec<(String, String)> = k.elements.iter().filter_map(|e| match &e.kind {
            anicraft_doc::ElementKind::Symbol { symbol, .. } => Some((doc.symbol(*symbol).map_or(String::new(), |s| s.name.clone()), e.name.clone())),
            _ => None,
        }).collect();
        let twin: Vec<(String, String, bool)> = kids(fr, "elements").flat_map(|els| kids(els, "DOMSymbolInstance")).map(|e| {
            let n = e.attr("libraryItemName").unwrap_or("");
            (n.rsplit('/').next().unwrap_or(n).to_string(), e.attr("name").unwrap_or("").to_string(), e.attr("symbolType") == Some("button"))
        }).collect();
        if legacy.len() != twin.len() {
            mismatches.push(format!("{tag}: {} instances vs twin {}", legacy.len(), twin.len()));
            return;
        }
        for ((ls, ln), (ts, tn, button)) in legacy.iter().zip(&twin) {
            instances += 1;
            named += usize::from(!tn.is_empty());
            buttons += usize::from(*button);
            if ls == ts && ln == tn {
                same += 1;
            } else {
                mismatches.push(format!("{tag}: {ls} named {ln:?} vs twin {ts} named {tn:?}{}", if *button { " (button)" } else { "" }));
            }
        }
    });
    // Symbol types: Contents' type byte against the twin's `symbolType`.
    let mut kinds = 0;
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let (doc, _) = anicraft_fla::import(&std::fs::read(&legacy_path).expect("reads")).expect("imports");
        for m in twin_members(&twin_path).into_iter().filter(|m| m.starts_with("LIBRARY/") && m.ends_with(".xml")) {
            let path = m.trim_start_matches("LIBRARY/").trim_end_matches(".xml").to_string();
            let Some(sym) = doc.library.iter().find(|s| doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Symbol(s.id))) == path) else { continue };
            let Some(xml) = twin_member(&twin_path, &m) else { continue };
            let Ok(x) = craft_xfl::document::Document::parse(&xml) else { continue };
            let want = match x.root().attr("symbolType") {
                Some("graphic") => anicraft_doc::SymbolKind::Graphic,
                Some("button") => anicraft_doc::SymbolKind::Button,
                _ => anicraft_doc::SymbolKind::MovieClip,
            };
            kinds += 1;
            if sym.kind != want {
                mismatches.push(format!("{legacy}: {path} is {:?}, twin {want:?}", sym.kind));
            }
        }
    }
    eprintln!("instances: {same}/{instances} same symbol and name ({named} named, {buttons} buttons); {kinds} symbol types compared");
    for m in mismatches.iter().take(30) {
        eprintln!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} instance or symbol differences", mismatches.len());
    assert!(buttons >= 18 && named >= 3, "the corpus's 18 button instances, 3 named CC (got {buttons}, {named})");
    assert!(kinds >= 300, "expected the twins' symbols by path, compared {kinds}");
}

#[test]
fn the_corpus_library_folders_match_the_twins() {
    let Some(dir) = corpus() else { return };
    let (mut folders, mut items) = (0usize, 0usize);
    let mut mismatches = Vec::new();
    for (legacy, twin) in TWINS {
        let (legacy_path, twin_path) = (dir.join(legacy), dir.join(twin));
        if !legacy_path.exists() || !twin_path.exists() {
            continue;
        }
        let (doc, _) = anicraft_fla::import(&std::fs::read(&legacy_path).expect("reads")).expect("imports");
        let dom = twin_member(&twin_path, "DOMDocument.xml").expect("DOMDocument.xml");
        let dom = craft_xfl::document::Document::parse(&dom).expect("parses");
        let mut want: Vec<String> = dom.root().descendants().filter(|e| local(&e.name) == "DOMFolderItem").filter_map(|e| e.attr("name").map(str::to_string)).collect();
        let mut got: Vec<String> = doc.library_tree.folders.iter().map(|f| {
            let chain = doc.folder_chain(f.id);
            chain.iter().filter_map(|id| doc.folder(*id).map(|x| x.name.clone())).collect::<Vec<_>>().join("/")
        }).collect();
        want.sort();
        got.sort();
        folders += want.len();
        if want != got {
            mismatches.push(format!("{legacy}: folders {got:?} vs twin {want:?}"));
        }
        // Every symbol whose twin file has a unique name sits at the twin's path.
        let members: Vec<String> = twin_members(&twin_path).into_iter().filter(|m| m.starts_with("LIBRARY/") && m.ends_with(".xml")).collect();
        for sym in &doc.library {
            let hits: Vec<&String> = members.iter().filter(|m| m.rsplit('/').next() == Some(format!("{}.xml", sym.name).as_str())).collect();
            let [m] = hits.as_slice() else { continue };
            items += 1;
            let want = m.trim_start_matches("LIBRARY/").trim_end_matches(".xml");
            let got = doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Symbol(sym.id)));
            if got != want {
                mismatches.push(format!("{legacy}: symbol at {got:?}, twin {want:?}"));
            }
        }
    }
    eprintln!("library folders: {folders} twin folders compared, {items} symbols at their twin's path");
    for m in mismatches.iter().take(30) {
        eprintln!("  {m}");
    }
    assert!(mismatches.is_empty(), "{} library differences", mismatches.len());
    assert!(folders >= 30 && items >= 300);
}
