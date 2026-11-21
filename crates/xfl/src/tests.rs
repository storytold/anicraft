//! Import tests on a small, self-authored XFL package.

use crate::import::{XflError, import_with};
use anicraft_doc::{ColorEffect, ColorTransform, ElementKind, LayerKind, LoopMode, Paint, Rgba, SymbolKind, evaluate};
use std::collections::HashMap;

#[test]
fn shared_zip_errors_keep_anicraft_import_wording() {
    let entry = XflError::from(crate::zip::ZipError::Unsupported("a.xml".into(), "encryption"));
    assert_eq!(entry.to_string(), "'a.xml' uses a zip feature AniCraft can't read (encryption)");
    let archive = XflError::from(crate::zip::ZipError::UnsupportedArchive("split across several files"));
    assert_eq!(archive.to_string(), "the archive uses a zip feature AniCraft can't read (split across several files)");
}

const DOC: &str = r##"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="640" height="360" frameRate="30" backgroundColor="#336699">
  <symbols><Include href="Characters/Ball.xml"/><Include href="Missing.xml"/></symbols>
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Ball" color="#FF4FFF">
          <frames>
            <DOMFrame index="0" duration="10" tweenType="motion" acceleration="-50" name="start">
              <elements>
                <DOMSymbolInstance libraryItemName="Characters/Ball" loop="play once" firstFrame="2" blendMode="hardlight">
                  <matrix><Matrix a="2" d="2" tx="100" ty="50"/></matrix>
                  <color><Color alphaMultiplier="0.5"/></color>
                  <filters><DropShadowFilter distance="8" color="#333333" alpha="0.5"/><BlurFilter blurX="10" blurY="10" quality="2"/><GlowFilter enabled="false"/>
                    <BevelFilter blurX="4" blurY="6" distance="3" angle="135" highlightColor="#FFFF00" highlightAlpha="0.75" shadowAlpha="0.5" type="full" knockout="true"/>
                    <GradientGlowFilter strength="2" type="inner"><GradientEntry color="#FFFFFF" alpha="0.5" ratio="0"/><GradientEntry color="#FF0000" ratio="1"/><GradientEntry color="#00FF00" ratio="128"/><GradientEntry color="#0000FF" ratio="255"/></GradientGlowFilter>
                    <GradientBevelFilter quality="3"><GradientEntry color="#FFFFFF" ratio="0"/><GradientEntry color="#000000" ratio="1"/></GradientBevelFilter>
                    <SparkleFilter/></filters>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="10" duration="5">
              <Actionscript><script><![CDATA[stop();]]></script></Actionscript>
              <elements>
                <DOMSymbolInstance libraryItemName="Characters/Ball"><matrix><Matrix a="2" d="2" tx="300" ty="50"/></matrix></DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="Ground" locked="true" heightMultiplier="2">
          <frames>
            <DOMFrame index="0" duration="15">
              <elements>
                <DOMShape>
                  <fills><FillStyle index="1"><SolidColor color="#00AA00"/></FillStyle>
                         <FillStyle index="2"><LinearGradient><matrix><Matrix a="0.1" d="0.1" tx="320"/></matrix>
                           <GradientEntry color="#000000" ratio="0"/><GradientEntry color="#FFFFFF" ratio="1"/></LinearGradient></FillStyle></fills>
                  <strokes><StrokeStyle index="1"><SolidStroke weight="3"><fill><SolidColor color="#112233"/></fill></SolidStroke></StrokeStyle></strokes>
                  <edges>
                    <Edge fillStyle1="1" strokeStyle="1" edges="!0 6000|12800 6000|12800 7200"/>
                    <Edge fillStyle0="1" edges="!0 6000|0 7200|12800 7200"/>
                  </edges>
                </DOMShape>
                <DOMStaticText/>
                <DOMBitmapInstance libraryItemName="photo.png"/>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="Notes" layerType="guide"><frames><DOMFrame index="0"><elements/></DOMFrame></frames></DOMLayer>
        <DOMLayer name="Folder" layerType="folder"/>
      </layers>
    </DOMTimeline>
    <DOMTimeline name="Scene 2"/>
  </timelines>
</DOMDocument>"##;

const BALL: &str = r##"<DOMSymbolItem name="Characters/Ball" symbolType="movie clip">
  <timeline><DOMTimeline name="Ball"><layers><DOMLayer name="Layer 1"><frames><DOMFrame index="0" duration="4"><Actionscript><script><![CDATA[gotoAndPlay(2);]]></script></Actionscript><elements>
    <DOMOvalObject x="-10" y="-10" objectWidth="20" objectHeight="20"><fill><SolidColor color="#FF0000"/></fill></DOMOvalObject>
  </elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline>
</DOMSymbolItem>"##;

fn package() -> HashMap<&'static str, &'static str> {
    HashMap::from([("DOMDocument.xml", DOC), ("LIBRARY/Characters/Ball.xml", BALL)])
}

fn import(files: &HashMap<&'static str, &'static str>) -> Result<(anicraft_doc::Document, crate::Report), XflError> {
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).map(|s| s.as_bytes().to_vec())) };
    import_with(&read)
}

#[test]
fn stage_layers_frames_and_tweens() {
    let (doc, report) = import(&package()).unwrap();
    assert_eq!((doc.stage.width, doc.stage.height, doc.stage.fps), (640, 360, 30.0));
    assert_eq!(doc.stage.background, Rgba::hex(0x336699));
    let names: Vec<&str> = doc.timeline(0).layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Ball", "Ground", "Notes", "Folder"], "folders come across as folders, order kept (top first)");
    assert_eq!(doc.timeline(0).layers[3].kind, LayerKind::Folder);
    assert!(doc.timeline(0).layers[3].keyframes.is_empty(), "a folder has no frames");
    let ball = &doc.timeline(0).layers[0];
    assert_eq!(ball.keyframes.len(), 2);
    assert_eq!((ball.keyframes[0].start, ball.keyframes[0].duration), (0, 10));
    assert_eq!(ball.keyframes[0].label, "start");
    assert!(matches!(ball.keyframes[0].tween.as_ref().map(|t| t.easing.clone()), Some(anicraft_doc::Easing::Classic(e)) if e == -50.0));
    assert!(doc.timeline(0).layers[1].locked);
    assert_eq!((doc.timeline(0).layers[0].height(), doc.timeline(0).layers[1].height()), (1, 2), "Layer Properties height");
    assert_eq!(doc.timeline(0).layers[2].kind, LayerKind::Guide);
    assert_eq!(doc.timeline(0).frame_count(), 15);
    assert_eq!(doc.frame_count(), 16, "the movie: scene 1, then the one-frame scene 2");
    // What couldn't come across is reported, not guessed.
    assert!(report.skipped.keys().all(|k| !k.contains("text")), "an empty text field makes nothing and reports nothing");
    assert_eq!(report.skipped.get("instances of missing bitmaps"), Some(&1));
    assert_eq!(report.skipped.get("missing library files"), Some(&1));
    assert_eq!(report.skipped.get("unknown filters"), Some(&1));
    // Frame scripts, on the scene's frames and inside symbols, are kept verbatim (ADR 0013
    // tier 1); these are in the subset that runs (tier 2).
    assert_eq!(report.runs.get("frame scripts in ActionScript 3.0"), Some(&2));
    // The symbol's path puts it in a Library folder (ADR 0050).
    assert_eq!(report.kept.keys().collect::<Vec<_>>(), ["library folders"], "{report:?}");
    assert_eq!(doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Symbol(doc.library[0].id))), "Characters/Ball");
    assert!(report.skipped.keys().all(|k| !k.contains("script")), "{report:?}");
    assert_eq!(ball.keyframes[1].script.as_ref().and_then(|s| s.source()).map(|s| s.text.as_str()), Some("stop();"));
    let inner = &doc.library[0].timeline.layers[0].keyframes[0];
    assert!(matches!(inner.script.as_ref().and_then(|s| s.source()), Some(s) if s.text == "gotoAndPlay(2);" && s.language == anicraft_doc::Language::ActionScript3));
    assert!(report.summary().contains("Runs in Test Movie: 2 frame scripts in ActionScript 3.0."), "{}", report.summary());
    // Every scene comes across, in order.
    assert_eq!(doc.scenes.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Scene 1", "Scene 2"]);
    assert_eq!(doc.timeline(1).layers.len(), 1, "an empty scene gets a layer to draw on");
}

#[test]
fn prefixed_xfl_names_survive_shared_parse_and_domain_mapping() {
    let doc = r##"<a:DOMDocument xmlns:a="http://ns.adobe.com/xfl/2008/" width="120" height="80">
      <a:timelines><a:DOMTimeline name="Prefixed Scene"><a:layers>
        <a:DOMLayer name="Prefixed Layer"><a:frames><a:DOMFrame index="0" duration="3" name="key">
          <a:elements><a:DOMRectangleObject x="2" y="3" objectWidth="10" objectHeight="8"><a:fill><a:SolidColor color="#336699"/></a:fill></a:DOMRectangleObject></a:elements>
        </a:DOMFrame></a:frames></a:DOMLayer>
      </a:layers></a:DOMTimeline></a:timelines>
    </a:DOMDocument>"##;
    let (document, _) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    assert_eq!(document.scenes[0].name, "Prefixed Scene");
    let layer = &document.timeline(0).layers[0];
    assert_eq!(layer.name, "Prefixed Layer");
    assert_eq!(layer.keyframes.len(), 1);
    assert_eq!(
        (layer.keyframes[0].start, layer.keyframes[0].duration, layer.keyframes[0].label.as_str()),
        (0, 3, "key")
    );
    assert_eq!(layer.keyframes[0].elements.len(), 1, "prefixed geometry reaches the app model");
}

#[test]
fn mask_layers_mask_the_layers_that_point_at_them() {
    // A mask inside a folder: `parentLayerIndex` counts every layer in the file, folders too.
    let shape = r##"<DOMShape><fills><FillStyle index="1"><SolidColor color="#FF0000"/></FillStyle></fills>
      <edges><Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/></edges></DOMShape>"##;
    let layer = |attrs: &str| format!(r#"<DOMLayer {attrs}><frames><DOMFrame index="0"><elements>{shape}</elements></DOMFrame></frames></DOMLayer>"#);
    let doc = format!(
        r#"<DOMDocument width="100" height="100"><timelines><DOMTimeline name="Scene 1"><layers>
        <DOMLayer name="Group" layerType="folder"/>{}{}{}{}</layers></DOMTimeline></timelines></DOMDocument>"#,
        layer(r#"name="Spot" layerType="mask" parentLayerIndex="0""#),
        layer(r#"name="Ball" layerType="masked" parentLayerIndex="1""#),
        layer(r#"name="Ring" parentLayerIndex="1""#),
        layer(r#"name="Sky" parentLayerIndex="0""#),
    );
    let doc: &'static str = Box::leak(doc.into_boxed_str());
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    let l = &d.timeline(0).layers;
    assert_eq!(l.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Group", "Spot", "Ball", "Ring", "Sky"]);
    assert_eq!((l[0].kind, l[1].kind), (LayerKind::Folder, LayerKind::Mask));
    assert_eq!(l[1].parent, Some(l[0].id), "the mask is in the folder");
    assert_eq!((l[2].parent, l[3].parent), (Some(l[1].id), Some(l[1].id)), "masked");
    assert_eq!(l[4].parent, Some(l[0].id), "in the folder, not masked");
    assert!(report.skipped.keys().all(|k| !k.contains("mask")));
    let dl = evaluate(&d, 0);
    assert_eq!(dl.clip_parents.len(), 1);
    assert_eq!(dl.items.iter().filter(|i| i.clip == Some(0)).count(), 2);
    assert_eq!(dl.items.iter().filter(|i| i.masks == Some(0)).count(), 1);
}

#[test]
fn symbols_instances_and_the_tween_between_them() {
    let (doc, _) = import(&package()).unwrap();
    assert_eq!(doc.library.len(), 1);
    let sym = &doc.library[0];
    assert_eq!((sym.name.as_str(), sym.kind), ("Ball", SymbolKind::MovieClip));
    let inst = &doc.timeline(0).layers[0].keyframes[0].elements[0];
    let ElementKind::Symbol { symbol, loop_mode, first_frame, blend, filters: entries, .. } = &inst.kind else { panic!("an instance") };
    assert_eq!(*blend, anicraft_doc::BlendMode::HardLight, "Animate's blend mode names");
    // In order; the switched-off glow comes in switched off, keeping its place (ADR 0091).
    assert_eq!(entries.iter().map(|e| e.enabled).collect::<Vec<_>>(), [true, true, false, true, true, true]);
    assert!(matches!(entries[2].filter, anicraft_doc::Filter::Glow { .. }));
    let filters: Vec<anicraft_doc::Filter> = entries.iter().filter(|e| e.enabled).map(|e| e.filter.clone()).collect();
    assert_eq!(filters.len(), 5, "{filters:?}");
    assert!(matches!(filters[0], anicraft_doc::Filter::DropShadow { distance, inner: false, .. } if distance == 8.0));
    assert!(matches!(filters[1], anicraft_doc::Filter::Blur { blur_x, quality: 2, .. } if blur_x == 10.0));
    use anicraft_doc::{Filter, FilterSide};
    let Filter::Bevel { blur_x, blur_y, distance, angle, highlight_color, shadow_color, side, knockout, strength, .. } = filters[2] else { panic!("{:?}", filters[2]) };
    assert_eq!((blur_x, blur_y, distance, angle, side, knockout, strength), (4.0, 6.0, 3.0, 135.0, FilterSide::Full, true, 1.0));
    assert_eq!((highlight_color, shadow_color), (Rgba::hex(0xffff00).with_alpha(0.75), Rgba::BLACK.with_alpha(0.5)));
    // Ramps: ratios 0..1 or 0..255; the glow's start pinned transparent, the bevel given a
    // transparent middle (as Animate's panel keeps them).
    let Filter::GradientGlow { stops, side: FilterSide::Inner, strength, .. } = &filters[3] else { panic!("{:?}", filters[3]) };
    assert_eq!(*strength, 2.0);
    assert_eq!(stops.iter().map(|s| (s.offset, s.color.a)).collect::<Vec<_>>(), vec![(0.0, 0.0), (1.0 / 255.0, 1.0), (128.0 / 255.0, 1.0), (1.0, 1.0)], "one byte scale for every ratio");
    let Filter::GradientBevel { stops, side: FilterSide::Inner, quality: 3, .. } = &filters[4] else { panic!("{:?}", filters[4]) };
    assert_eq!(stops.len(), 3);
    assert_eq!((stops[1].offset, stops[1].color.a), (0.5, 0.0));
    assert_eq!((*symbol, *loop_mode, *first_frame), (sym.id, LoopMode::PlayOnce, 2));
    assert_eq!(inst.transform.m, [2.0, 0.0, 0.0, 2.0, 100.0, 50.0]);
    assert_eq!(inst.color, anicraft_doc::ColorEffect::Alpha(0.5));
    // Half-way through the tween (eased), the ball is between its two positions.
    let dl = evaluate(&doc, 5);
    let ball = dl.items.iter().find(|i| matches!(i.fill, Some(Paint::Solid(c)) if c == Rgba::hex(0xff0000))).expect("the ball renders");
    let x = ball.transform.translation().x;
    assert!(x > 100.0 && x < 300.0, "{x}");
}

#[test]
fn shared_native_color_effect_view_preserves_anicraft_style_mapping() {
    let document = r##"<DOMDocument>
      <symbols><Include href="Target.xml"/></symbols>
      <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Layer 1"><frames><DOMFrame index="0"><elements>
        <DOMSymbolInstance libraryItemName="Target" name="Tint"><color><Color tintMultiplier=".25" tintColor="#336699" brightness=".8" alphaMultiplier=".5"/></color></DOMSymbolInstance>
        <DOMSymbolInstance libraryItemName="Target" name="Bright"><color><Color brightness="-.25" alphaMultiplier=".75"/></color></DOMSymbolInstance>
        <DOMSymbolInstance libraryItemName="Target" name="Advanced"><color><Color redMultiplier="2" greenMultiplier="bad" blueMultiplier="-.5" alphaMultiplier=".5" redOffset="255" greenOffset="-127.5" alphaOffset="128"/></color></DOMSymbolInstance>
      </elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timelines>
    </DOMDocument>"##;
    let symbol = r#"<DOMSymbolItem name="Target"><timeline><DOMTimeline name="Target"/></timeline></DOMSymbolItem>"#;
    let files = HashMap::from([("DOMDocument.xml", document), ("LIBRARY/Target.xml", symbol)]);
    let (doc, _) = import(&files).unwrap();
    let elements = &doc.timeline(0).layers[0].keyframes[0].elements;

    let mut tint = ColorTransform::tint(Rgba::hex(0x336699), 0.25);
    tint.mul[3] = 0.5;
    assert_eq!(elements[0].name, "Tint");
    assert_eq!(elements[0].color, ColorEffect::Advanced(tint).sanitized(), "tint keeps precedence over brightness and composes alpha");

    let bright = ColorEffect::Brightness(-0.25).with_alpha(0.75);
    assert_eq!(elements[1].name, "Bright");
    assert_eq!(elements[1].color, bright.sanitized());

    let advanced = ColorTransform { mul: [2.0, 1.0, -0.5, 0.5], add: [1.0, -0.5, 0.0, 128.0 / 255.0] };
    assert_eq!(elements[2].name, "Advanced");
    assert_eq!(
        elements[2].color,
        ColorEffect::Advanced(advanced).sanitized(),
        "invalid multipliers default to identity and offsets stay in XFL byte units until mapping"
    );
}

#[test]
fn bevel_filter_keeps_white_highlight_when_only_alpha_is_stored() {
    let mut files = package();
    let document = Box::leak(DOC.replace("<filters>", "<filters><BevelFilter highlightAlpha=\"0.4\"/>").into_boxed_str());
    files.insert("DOMDocument.xml", document);
    let (doc, _) = import(&files).unwrap();
    let ElementKind::Symbol { filters, .. } = &doc.timeline(0).layers[0].keyframes[0].elements[0].kind else { panic!("a symbol instance") };
    let anicraft_doc::Filter::Bevel { highlight_color, .. } = filters[0].filter else { panic!("a bevel filter") };
    assert_eq!(highlight_color, Rgba::WHITE.with_alpha(0.4));
}

#[test]
fn shapes_become_fills_and_strokes() {
    let (doc, _) = import(&package()).unwrap();
    let els = &doc.timeline(0).layers[1].keyframes[0].elements;
    assert_eq!(els.len(), 2, "one fill (style 2 has no edges), one stroke");
    let ElementKind::Shape(fill) = &els[0].kind else { panic!() };
    assert_eq!(fill.fill, Some(Paint::Solid(Rgba::hex(0x00aa00))));
    let b = fill.path.bounds();
    assert!((b.min.y - 300.0).abs() < 0.01 && (b.max.x - 640.0).abs() < 0.01 && (b.max.y - 360.0).abs() < 0.01, "{b:?}: twips to pixels");
    let ElementKind::Shape(st) = &els[1].kind else { panic!() };
    assert_eq!(st.stroke.as_ref().map(|s| (s.color, s.width)), Some((Rgba::hex(0x112233), 3.0)));
    assert!(st.fill.is_none());
}

#[test]
fn a_fla_archive_imports_the_same() {
    let z = crate::zip::pack::write(&[crate::zip::pack::Raw::new("DOMDocument.xml", DOC.as_bytes(), true), crate::zip::pack::Raw::new("LIBRARY/Characters/Ball.xml", BALL.as_bytes(), true)], false);
    let dir = std::env::temp_dir().join(format!("anicraft-xfl-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fla = dir.join("scene.fla");
    std::fs::write(&fla, &z).unwrap();
    let (doc, _) = crate::import_path(&fla).unwrap();
    assert_eq!(doc.timeline(0).layers.len(), 4);
    assert_eq!(doc.library.len(), 1);
    let (from_bytes, _) = crate::import_fla(&z).unwrap();
    assert_eq!((from_bytes.timeline(0).layers.len(), from_bytes.library.len()), (4, 1), "the bytes a browser hands over import the same");
    assert!(crate::import_fla(b"not a zip").is_err());
    std::fs::remove_file(&fla).ok();
    std::fs::remove_dir(&dir).ok();
}

#[test]
fn folder_import_ignores_backslash_escapes_and_symlinks() {
    let dir = std::env::temp_dir().join(format!("anicraft-xfl-path-{}", std::process::id()));
    let pkg = dir.join("pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(dir.join("secret.xml"), DOC).unwrap();
    // A reference that climbs out with backslashes is not read (here it would be the whole document).
    std::fs::write(pkg.join("DOMDocument.xml"), "<DOMDocument><symbols><Include href=\"x\\..\\..\\secret.xml\"/></symbols></DOMDocument>").unwrap();
    let (doc, _) = crate::import_path(&pkg).unwrap();
    assert_eq!(doc.library.len(), 0);
    #[cfg(unix)]
    {
        std::fs::remove_file(pkg.join("DOMDocument.xml")).unwrap();
        std::os::unix::fs::symlink(dir.join("secret.xml"), pkg.join("DOMDocument.xml")).unwrap();
        let e = crate::import_path(&pkg).err().map(|e| e.to_string());
        assert!(e.is_some_and(|e| e.contains("no DOMDocument.xml")), "a symlink is not package content");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hostile_packages_fail_cleanly() {
    let mut files = package();
    files.insert("DOMDocument.xml", "<DOMDocument><timelines><DOMTimeline><layers><DOMLayer><frames><DOMFrame index=\"99999999999\" duration=\"-4\"><elements><DOMShape><edges><Edge fillStyle1=\"1\" edges=\"!#FFFFFFFF.FF 1e30|nan 0[\"/></edges></DOMShape></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>");
    let (doc, _) = import(&files).unwrap();
    assert!(doc.frame_count() <= 100_001);
    files.insert("DOMDocument.xml", "<NotAnimate/>");
    assert!(matches!(import(&files), Err(XflError::NotXfl(_))));
    files.insert("DOMDocument.xml", "<DOMDocument><unclosed>");
    assert!(matches!(import(&files), Err(XflError::Xml(_))));
    assert!(import(&HashMap::new()).is_err(), "no DOMDocument.xml");
}

#[test]
fn shape_tweens_come_across_as_shape_tweens() {
    let shape = |edges: &str| {
        format!(r##"<DOMShape><fills><FillStyle index="1"><SolidColor color="#3366FF"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="{edges}"/></edges></DOMShape>"##)
    };
    let doc = format!(
        r#"<DOMDocument width="400" height="200"><timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Morph"><frames>
          <DOMFrame index="0" duration="10" tweenType="shape" acceleration="40"><elements>{}</elements></DOMFrame>
          <DOMFrame index="10"><elements>{}</elements></DOMFrame>
        </frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#,
        shape("!0 0|2000 0|2000 2000|0 2000|0 0"),
        shape("!4000 0|6000 1000|4000 2000|4000 0"),
    );
    let doc: &'static str = Box::leak(doc.into_boxed_str());
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    let k = &d.timeline(0).layers[0].keyframes[0];
    let t = k.tween.clone().expect("a tween");
    assert_eq!(t.kind, anicraft_doc::TweenKind::Shape);
    assert!(matches!(t.easing, anicraft_doc::Easing::Classic(a) if a == 40.0));
    assert!(report.skipped.keys().all(|s| !s.contains("shape tween")), "{:?}", report.skipped);
    // Halfway, the square has morphed towards the triangle.
    let mid = evaluate(&d, 5).items[0].path.bounds();
    assert!(mid.max.x > 110.0 && mid.min.x > 10.0, "{mid:?}");
}

fn png_2x1() -> Vec<u8> {
    let img = craft_image_codecs::Image::from_u8(2, 1, craft_image_codecs::ChannelLayout::Rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap()
}

const BITMAP_DOC: &str = r##"<DOMDocument width="200" height="100">
  <folders><DOMFolderItem name="Art" isExpanded="true"/><DOMFolderItem name="Empty/Inner"/></folders>
  <media>
    <DOMBitmapItem name="Art/photo.png" href="Art/photo.png" bitmapDataHRef="M 1 1.dat"/>
    <DOMBitmapItem name="packed.png" href="packed.png" bitmapDataHRef="M 2 2.dat" allowSmoothing="false"/>
    <DOMBitmapItem name="private.jpg" href="private.jpg" bitmapDataHRef="M 3 3.dat"/>
    <DOMBitmapItem name="gone.png" href="gone.png"/>
  </media>
  <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Layer 1"><frames><DOMFrame index="0"><elements>
    <DOMBitmapInstance libraryItemName="Art/photo.png"><matrix><Matrix a="10" d="10" tx="5" ty="6"/></matrix></DOMBitmapInstance>
    <DOMBitmapInstance libraryItemName="packed.png"/>
    <DOMBitmapInstance libraryItemName="gone.png"/>
    <DOMShape>
      <fills><FillStyle index="1"><BitmapFill bitmapPath="Art/photo.png"><matrix><Matrix a="40" d="40" tx="100" ty="50"/></matrix></BitmapFill></FillStyle>
             <FillStyle index="2"><BitmapFill bitmapPath="private.jpg" bitmapIsClipped="true"/></FillStyle></fills>
      <edges><Edge fillStyle1="1" edges="!2000 1000|4000 1000|4000 2000|2000 2000|2000 1000"/>
             <Edge fillStyle1="2" edges="!0 0|200 0|200 200|0 0"/></edges>
    </DOMShape>
  </elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timelines>
</DOMDocument>"##;

#[test]
fn bitmaps_come_across_when_their_image_is_png_or_jpeg() {
    let files: HashMap<String, Vec<u8>> = HashMap::from([
        ("DOMDocument.xml".to_string(), BITMAP_DOC.as_bytes().to_vec()),
        // An XFL folder keeps the original image in LIBRARY/.
        ("LIBRARY/Art/photo.png".to_string(), png_2x1()),
        // A .fla may hold only the data file; a PNG or JPEG there is used as it is.
        ("bin/M 2 2.dat".to_string(), png_2x1()),
        // Animate's own layout in a pixel format not yet seen: reported, not guessed.
        ("bin/M 3 3.dat".to_string(), [3u8, 3].into_iter().chain([0u8; 24]).collect()),
    ]);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = import_with(&read).unwrap();
    assert_eq!(doc.bitmaps.iter().map(|b| (b.name.as_str(), b.width, b.height)).collect::<Vec<_>>(), [("photo.png", 2, 1), ("packed.png", 2, 1)]);
    assert_eq!(report.names.get("bitmaps in Animate's own format AniCraft can't decode"), Some(&vec!["private.jpg: bitmap pixel format 3, which AniCraft doesn't read yet".to_string()]));
    assert_eq!(report.skipped.get("missing bitmap files"), Some(&1));
    assert_eq!(report.skipped.get("instances of missing bitmaps"), Some(&1));
    assert_eq!(report.skipped.get("bitmap fills of missing bitmaps"), Some(&1));

    let els = &doc.timeline(0).layers[0].keyframes[0].elements;
    assert_eq!(els.len(), 3, "two bitmap instances and the bitmap-filled shape");
    let photo = doc.bitmaps[0].id;
    // An instance is a rectangle of the image's size, placed by its matrix.
    let ElementKind::Shape(sh) = &els[0].kind else { panic!() };
    assert_eq!(sh.fill, Some(Paint::Bitmap { bitmap: photo, repeat: false, matrix: anicraft_geom::Affine::IDENTITY }));
    assert_eq!(sh.path.bounds(), anicraft_geom::Rect::new(0.0, 0.0, 2.0, 1.0));
    assert_eq!(els[0].transform.m, [10.0, 0.0, 0.0, 10.0, 5.0, 6.0]);
    // A bitmap fill keeps its matrix, in pixels (twips scale removed), and repeats.
    let ElementKind::Shape(fill) = &els[2].kind else { panic!() };
    assert_eq!(fill.fill, Some(Paint::Bitmap { bitmap: photo, repeat: true, matrix: anicraft_geom::Affine { m: [2.0, 0.0, 0.0, 2.0, 100.0, 50.0] } }));
    // The renderer gets the pixels with the items.
    let list = evaluate(&doc, 0);
    assert_eq!(list.items.iter().filter(|i| i.image.is_some()).count(), 3);
    // Library folders come across (ADR 0050): the item's path places it, empty ones are kept,
    // and Allow Smoothing is read (on unless the file turns it off).
    let path = |i| doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Bitmap(i)));
    assert_eq!((path(doc.bitmaps[0].id).as_str(), path(doc.bitmaps[1].id).as_str()), ("Art/photo.png", "packed.png"));
    let folders: Vec<String> = doc.library_tree.folders.iter().map(|f| doc.entry_path(anicraft_doc::LibraryEntry::Folder(f.id))).collect();
    assert_eq!(folders, ["Art", "Empty", "Empty/Inner"]);
    assert_eq!(report.kept.get("library folders"), Some(&1));
    assert_eq!((doc.bitmaps[0].smoothing, doc.bitmaps[1].smoothing), (true, false));
    assert!(list.items.iter().any(|i| i.image.is_some() && !i.smooth));
}

/// A square (0, 0)–(100, 100) shape-tweening to a triangle (200, 0), (300, 50), (200, 100),
/// with `first` and `second` inside the two frames.
fn hinted_doc(first: &str, second: &str) -> &'static str {
    let shape = |edges: &str| {
        format!(r##"<DOMShape><fills><FillStyle index="1"><SolidColor color="#3366FF"/></FillStyle></fills><edges><Edge fillStyle1="1" edges="{edges}"/></edges></DOMShape>"##)
    };
    let doc = format!(
        r#"<DOMDocument width="400" height="200"><timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Morph"><frames>
          <DOMFrame index="0" duration="10" tweenType="shape">{first}<elements>{}</elements></DOMFrame>
          <DOMFrame index="10">{second}<elements>{}</elements></DOMFrame>
        </frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#,
        shape("!0 0|2000 0|2000 2000|0 2000|0 0"),
        shape("!4000 0|6000 1000|4000 2000|4000 0"),
    );
    Box::leak(doc.into_boxed_str())
}

#[test]
fn shape_hints_come_across_in_either_form() {
    use anicraft_geom::{PathCmd, Vec2};
    let both_in_one = hinted_doc(r#"<shapeHints><ShapeHint index="0" startPoint="0, 0" endPoint="300, 50"/></shapeHints>"#, "");
    // One list per keyframe, matched by letter; the second point a pixel off the edge snaps on.
    let per_key = hinted_doc(r#"<shapeHints><ShapeHint name="a" x="0" y="0"/></shapeHints>"#, r#"<shapeHints><ShapeHint name="a" x="299" y="50.5"/></shapeHints>"#);
    for doc in [both_in_one, per_key] {
        let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
        assert!(report.skipped.keys().all(|s| !s.contains("hint")), "{:?}", report.skipped);
        let l = &d.timeline(0).layers[0];
        let k = &l.keyframes[0];
        assert_eq!(k.hints.len(), 1);
        assert!(anicraft_doc::placed_on(&k.elements, k.hints[0].start).is_some());
        assert!(anicraft_doc::placed_on(&l.keyframes[1].elements, k.hints[0].end).is_some(), "{:?}", k.hints[0].end);
        // Halfway the square's corner is halfway to the triangle's tip.
        let dl = evaluate(&d, 5);
        let want = Vec2::new(0.0, 0.0).lerp(k.hints[0].end, 0.5);
        let it = &dl.items[0];
        let hit = it.path.cmds.iter().any(|c| matches!(*c, PathCmd::MoveTo(a) | PathCmd::CubicTo(_, _, a) if it.transform.apply(a).distance(want) < 0.05));
        assert!(hit, "{want:?} {:?}", it.path.cmds);
    }
}

#[test]
fn shape_hints_come_across_as_animate_writes_them() {
    // `morphHintsList` on the start keyframe, both points per record, in units of 256 twips:
    // (0, 0) and (300, 50) px, the end as hex fixed point and decimal. The list inside
    // `MorphShape` is not the artist's and is left alone.
    let hint = r##"<MorphHint startPoint="0, #0" endPoint="#17.70, 3.90625" key="0"/>"##;
    let doc = hinted_doc(&format!("<MorphShape><morphHintsList>{hint}{hint}</morphHintsList></MorphShape><morphHintsList>{hint}</morphHintsList>"), "");
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    assert!(report.skipped.keys().all(|s| !s.contains("hint")), "{:?}", report.skipped);
    let l = &d.timeline(0).layers[0];
    let k = &l.keyframes[0];
    assert_eq!(k.hints.len(), 1);
    assert!(k.hints[0].start.distance(anicraft_geom::Vec2::new(0.0, 0.0)) < 1e-3, "{:?}", k.hints[0]);
    assert!(k.hints[0].end.distance(anicraft_geom::Vec2::new(300.0, 50.0)) < 1e-3, "{:?}", k.hints[0]);
    assert!(anicraft_doc::placed_on(&k.elements, k.hints[0].start).is_some());
    assert!(anicraft_doc::placed_on(&l.keyframes[1].elements, k.hints[0].end).is_some());
    // Either spelling gives the same hints.
    let native = hinted_doc(r#"<shapeHints><ShapeHint startPoint="0, 0" endPoint="300, 50"/></shapeHints>"#, "");
    let (n, _) = import(&HashMap::from([("DOMDocument.xml", native)])).unwrap();
    assert_eq!(n.timeline(0).layers[0].keyframes[0].hints, k.hints);
}

#[test]
fn odd_morph_hints_are_reported_not_fatal() {
    let many: String = (0..30).map(|_| r##"<MorphHint startPoint="0, 0" endPoint="#F.A0, 0"/>"##).collect();
    let odd = r##"<MorphHint/><MorphHint startPoint="#zz, 1"/><MorphHint startPoint="1e39, 0"/><MorphHint startPoint="#7FFFFFFF.FF, #80000000" endPoint="#F.A0, 0"/>"##;
    let doc = hinted_doc(&format!("<morphHintsList>{odd}{many}<Other startPoint=\"0, 0\"/></morphHintsList>"), "");
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    assert_eq!(d.timeline(0).layers[0].keyframes[0].hints.len(), 26);
    assert!(report.skipped.contains_key("shape hints past z"), "{:?}", report.skipped);
    assert!(report.skipped.contains_key("shape hints without a position"));
    // No end point, and no hints on the end keyframe to take one from.
    let doc = hinted_doc(r#"<morphHintsList><MorphHint startPoint="0, 0" endPoint="0"/></morphHintsList>"#, "");
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    assert!(d.timeline(0).layers[0].keyframes[0].hints.is_empty());
    assert!(report.skipped.contains_key("shape hints missing on the end keyframe"));
}

#[test]
fn odd_shape_hints_are_reported_not_fatal() {
    let many: String = (0..30).map(|i| format!(r#"<ShapeHint index="{i}" startPoint="0, 0" endPoint="200, 0"/>"#)).collect();
    let doc = hinted_doc(&format!("<shapeHints>{many}<ShapeHint/><ShapeHint x=\"nan\" y=\"1\"/></shapeHints>"), "");
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    assert_eq!(d.timeline(0).layers[0].keyframes[0].hints.len(), 26);
    assert!(report.skipped.contains_key("shape hints past z"), "{:?}", report.skipped);
    assert!(report.skipped.contains_key("shape hints without a position"));
    // Only start points and nothing on the end keyframe.
    let doc = hinted_doc(r#"<shapeHints><ShapeHint x="0" y="0"/></shapeHints>"#, "");
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    assert!(d.timeline(0).layers[0].keyframes[0].hints.is_empty());
    assert!(report.skipped.contains_key("shape hints missing on the end keyframe"));
}

#[test]
fn eases_names_and_loop_modes_the_corpus_found_missing() {
    let frame = |attrs: &str, inner: &str| format!(r#"<DOMFrame index="0" duration="4" tweenType="motion" {attrs}>{inner}<elements><DOMSymbolInstance libraryItemName="Clip" name="my clip" loop="loop reverse"/></elements></DOMFrame><DOMFrame index="4"><elements/></DOMFrame>"#);
    let pts = |p: &[(f32, f32)]| p.iter().map(|(x, y)| format!(r#"<Point x="{x}" y="{y}"/>"#)).collect::<String>();
    let layers = [
        // Seven points: a curve of two segments, exactly.
        frame(r#"hasCustomEase="true" acceleration="30""#, &format!(r#"<tweens><CustomEase target="all">{}</CustomEase></tweens>"#, pts(&[(0.0, 0.0), (0.2, 0.0), (0.3, 0.5), (0.5, 0.5), (0.7, 0.5), (0.8, 1.0), (1.0, 1.0)]))),
        // One curve per property: each its own; the slider eases the rest.
        frame(r#"hasCustomEase="true" useSingleEaseCurve="false" acceleration="-50""#, &format!(r#"<tweens><CustomEase target="position">{0}</CustomEase><CustomEase target="rotation">{1}</CustomEase><CustomEase target="sparkle">{0}</CustomEase></tweens>"#, pts(&[(0.0, 0.0), (0.5, 0.0), (0.5, 1.0), (1.0, 1.0)]), pts(&[(0.0, 0.0), (0.1, 0.4), (0.2, 1.2), (0.4, 1.2), (0.6, 1.2), (0.9, 1.0), (1.0, 1.0)]))),
        // Said to have a custom ease, with none there; and a broken point list.
        frame(r#"hasCustomEase="true""#, ""),
        frame(r#"hasCustomEase="true" acceleration="10""#, &format!(r#"<tweens><CustomEase target="all">{}</CustomEase></tweens>"#, pts(&[(0.0, 0.0), (0.5, 0.0), (1.0, 1.0)]))),
    ];
    let layers: String = layers.iter().map(|f| format!(r#"<DOMLayer name="L"><frames>{f}</frames></DOMLayer>"#)).collect();
    let doc = format!(r#"<DOMDocument><symbols><Include href="Clip.xml"/></symbols><timelines><DOMTimeline name="Scene 1"><layers>{layers}</layers></DOMTimeline></timelines></DOMDocument>"#);
    let doc: &'static str = Box::leak(doc.into_boxed_str());
    let clip = r#"<DOMSymbolItem name="Clip"><timeline><DOMTimeline name="Clip"/></timeline></DOMSymbolItem>"#;
    let (d, report) = import(&HashMap::from([("DOMDocument.xml", doc), ("LIBRARY/Clip.xml", clip)])).unwrap();
    use anicraft_doc::{CustomEase, EaseGroup, Easing};
    let tween = |l: usize| d.timeline(0).layers[l].keyframes[0].tween.clone().unwrap();
    let v = |p: &[(f32, f32)]| p.iter().map(|(x, y)| anicraft_geom::Vec2::new(*x, *y)).collect::<Vec<_>>();
    let two = CustomEase::from_flat(&v(&[(0.0, 0.0), (0.2, 0.0), (0.3, 0.5), (0.5, 0.5), (0.7, 0.5), (0.8, 1.0), (1.0, 1.0)])).unwrap();
    assert_eq!(tween(0).easing, Easing::Custom(std::sync::Arc::new(two)), "a two-segment curve, exactly");
    assert!((tween(0).easing.apply(0.5) - 0.5).abs() < 1e-5 && tween(0).easing.apply(0.25) < 0.4);
    let per = tween(1);
    assert_eq!(per.easing, Easing::Classic(-50.0));
    assert_eq!(per.ease(EaseGroup::Position), &Easing::CubicBezier(0.5, 0.0, 0.5, 1.0));
    assert!(matches!(per.ease(EaseGroup::Rotation), Easing::Custom(c) if c.points().len() == 3));
    assert_eq!((per.has_own(EaseGroup::Scale), per.groups.len()), (false, 2));
    assert_eq!((tween(2).easing, tween(3).easing), (Easing::Linear, Easing::Classic(10.0)));
    assert_eq!(report.skipped.get("custom eases of unknown properties (left out)"), Some(&1));
    assert_eq!(report.skipped.get("custom eases without their curve (drawn with the simple ease)"), Some(&1));
    assert_eq!(report.skipped.get("custom ease curves with a broken point list (drawn with the simple ease)"), Some(&1));
    assert!(!report.skipped.contains_key("custom ease curves (drawn with the simple ease)"));
    assert_eq!(d.library[0].kind, SymbolKind::MovieClip, "no symbolType: a movie clip");
    let ElementKind::Symbol { loop_mode, .. } = &d.timeline(0).layers[0].keyframes[0].elements[0].kind else { panic!() };
    assert_eq!(*loop_mode, LoopMode::Loop, "loop reverse plays forward");
    assert_eq!(report.skipped.get("reverse loop modes (played forward)"), Some(&4));
    assert_eq!(d.timeline(0).layers[0].keyframes[0].elements[0].name, "", "not an identifier: left out");
    assert_eq!(report.skipped.get("instance names actions can't use (left out)"), Some(&4));
}

#[test]
fn scripts_linkage_and_the_document_class_are_kept_not_run() {
    // ADR 0013 tier 1: frame scripts verbatim, linkage classes and the document class as text.
    let doc = r#"<DOMDocument><symbols><Include href="Hero.xml"/><Include href="Old.xml"/><Include href="Bad.xml"/><Include href="Off.xml"/></symbols>
      <media><DOMBitmapItem name="pic.png" linkageExportForAS="true" linkageClassName="Pic"/></media>
      <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="as"><frames>
        <DOMFrame index="0"><Actionscript><script><![CDATA[var n:int = 1 < 2 ? 1 : 0; // <tags> & "quotes"
trace(n);]]></script></Actionscript><elements><DOMSymbolInstance libraryItemName="Hero"><Actionscript><script><![CDATA[on (release) { play(); }]]></script></Actionscript></DOMSymbolInstance></elements></DOMFrame>
        <DOMFrame index="1"><Actionscript><script><![CDATA[   ]]></script></Actionscript><elements/></DOMFrame>
      </frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#;
    let sym = |name: &str, attrs: &str| format!(r#"<DOMSymbolItem name="{name}" {attrs}><timeline><DOMTimeline name="{name}"/></timeline></DOMSymbolItem>"#);
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let settings = r#"<flash_profiles><flash_profile name="Old" current="false"><PublishFlashProperties><ActionScriptVersion>3</ActionScriptVersion><DocumentClass>Nope</DocumentClass></PublishFlashProperties></flash_profile>
      <flash_profile name="Default" current="true"><PublishFlashProperties><ActionScriptVersion>2</ActionScriptVersion><DocumentClass> com.example.Main </DocumentClass></PublishFlashProperties></flash_profile></flash_profiles>"#;
    let files = HashMap::from([
        ("DOMDocument.xml", doc),
        ("PublishSettings.xml", settings),
        ("LIBRARY/Hero.xml", leak(sym("Hero", r#"linkageExportForAS="true" linkageClassName="com.example.Hero" linkageBaseClass="flash.display.MovieClip""#))),
        ("LIBRARY/Old.xml", leak(sym("Old", r#"linkageExportForAS="true" linkageIdentifier="oldClip""#))),
        ("LIBRARY/Bad.xml", leak(sym("Bad", r#"linkageExportForAS="true" linkageClassName="not a class""#))),
        ("LIBRARY/Off.xml", leak(sym("Off", r#"linkageExportForAS="false" linkageClassName="Off""#))),
    ]);
    let (d, report) = import(&files).unwrap();
    let s = d.timeline(0).layers[0].keyframes[0].script.as_ref().and_then(|s| s.source()).unwrap();
    assert_eq!(s.text, "var n:int = 1 < 2 ? 1 : 0; // <tags> & \"quotes\"\ntrace(n);", "verbatim");
    assert_eq!(s.language, anicraft_doc::Language::ActionScript1or2, "the current publish profile says ActionScript 2");
    assert!(d.timeline(0).layers[0].keyframes[1].script.is_none(), "a blank script is nothing");
    assert_eq!(d.document_class, "com.example.Main");
    let linkage: Vec<Option<(&str, &str)>> = d.library.iter().map(|s| s.linkage.as_ref().map(|l| (l.class.as_str(), l.base.as_str()))).collect();
    assert_eq!(linkage, [Some(("com.example.Hero", "flash.display.MovieClip")), Some(("oldClip", "")), None, None]);
    let kept: Vec<(&str, usize)> = report.kept.iter().map(|(k, n)| (k.as_str(), *n)).collect();
    assert_eq!(kept, [("document class", 1), ("frame scripts in ActionScript 1.0 & 2.0", 1), ("linkage classes", 2)]);
    assert_eq!(report.skipped.get("linkage classes that aren't class names (left out)"), Some(&1));
    assert_eq!(report.skipped.get("linkage classes on bitmaps and sounds (left out)"), Some(&1));
    assert_eq!(report.skipped.get("scripts on instances (left out; frame scripts are kept)"), Some(&1));
    // Broken or missing publish settings change nothing.
    let mut files = files;
    files.insert("PublishSettings.xml", "<flash_profiles><unclosed>");
    let (d, report) = import(&files).unwrap();
    assert!(d.document_class.is_empty() && !report.kept.contains_key("document class"));
    // Read as ActionScript 3.0, its variable and `?:` are in the subset now (ADR 0052).
    assert!(report.runs.contains_key("frame scripts in ActionScript 3.0"));
}

#[test]
fn shared_symbol_metadata_views_keep_prefixed_library_mapping_and_defaults() {
    let document = r#"<x:DOMDocument xmlns:x="http://ns.adobe.com/xfl/2008/">
      <x:symbols>
        <x:Extension href="Missing.xml"/>
        <x:Include href="Ui/Panel.xml"/>
        <x:Include href="Ui/Panel.xml"/>
      </x:symbols>
      <x:folders><x:Extension name="Ignored"/><x:DOMFolderItem name="Empty"/></x:folders>
      <x:timelines><x:DOMTimeline name="Scene 1"><x:layers/></x:DOMTimeline></x:timelines>
    </x:DOMDocument>"#;
    let symbol = r#"<x:DOMSymbolItem xmlns:x="http://ns.adobe.com/xfl/2008/" name="Ui/Panel" symbolType="button"
        linkageExportForAS="true" linkageClassName=" " linkageIdentifier="ui.Panel"
        linkageBaseClass="flash.display.MovieClip"
        scaleGridLeft="90" scaleGridRight="10" scaleGridTop="30" scaleGridBottom="10">
      <x:timeline><x:DOMTimeline name="Panel"><x:layers/></x:DOMTimeline></x:timeline>
    </x:DOMSymbolItem>"#;
    let files = HashMap::from([("DOMDocument.xml", document), ("LIBRARY/Ui/Panel.xml", symbol)]);
    let (doc, report) = import(&files).unwrap();

    assert_eq!(doc.library.len(), 1, "repeated Include paths still register one library symbol");
    let panel = &doc.library[0];
    assert_eq!(panel.name, "Panel");
    assert_eq!(panel.kind, SymbolKind::Button);
    assert_eq!(panel.linkage.as_ref().map(|linkage| (linkage.class.as_str(), linkage.base.as_str())), Some(("ui.Panel", "flash.display.MovieClip")));
    let grid = panel.scale_grid.expect("all four finite guide attributes form a grid");
    assert_eq!((grid.left, grid.right, grid.top, grid.bottom), (10.0, 90.0, 10.0, 30.0));
    let folder_paths: Vec<String> = doc.library_tree.folders.iter().map(|folder| doc.entry_path(anicraft_doc::LibraryEntry::Folder(folder.id))).collect();
    assert_eq!(folder_paths, ["Empty", "Ui"]);
    assert!(!report.skipped.contains_key("missing library files"), "non-Include extension nodes are ignored");
}

#[test]
fn loose_shapes_come_in_raw_and_drawing_objects_and_groups_as_groups() {
    // Animate's raw shapes (merge drawing) are loose `DOMShape`s; a drawing object says so, and
    // a group is a group (ADR 0073) whose shapes merge only with each other.
    let square = |attrs: &str, x: u32| {
        format!(
            r##"<DOMShape {attrs}><fills><FillStyle index="1"><SolidColor color="#FF0000"/></FillStyle></fills><strokes><StrokeStyle index="1"><SolidStroke weight="2"><fill><SolidColor/></fill></SolidStroke></StrokeStyle></strokes>
            <edges><Edge fillStyle1="1" strokeStyle="1" edges="!{x} 0|{} 0|{} 2000|{x} 2000|{x} 0"/></edges></DOMShape>"##,
            x + 2000,
            x + 2000
        )
    };
    let doc = format!(
        r#"<DOMDocument width="400" height="100"><timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Art"><frames><DOMFrame index="0"><elements>{}{}<DOMGroup><members>{}</members></DOMGroup></elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#,
        square("", 0),
        square(r#"isDrawingObject="true""#, 4000),
        square("", 8000),
    );
    let doc: &'static str = Box::leak(doc.into_boxed_str());
    let (d, _) = import(&HashMap::from([("DOMDocument.xml", doc)])).unwrap();
    let els = &d.timeline(0).layers[0].keyframes[0].elements;
    let merge: Vec<bool> = els.iter().map(|e| matches!(&e.kind, ElementKind::Shape(s) if s.merge)).collect();
    // Each square is a fill and a stroke; the third is in a group (ADR 0073), raw inside it.
    assert_eq!(merge, [true, true, false, false, false]);
    let group = els.last().unwrap();
    assert!(matches!(group.kind, ElementKind::Group { .. }));
    let inside: Vec<bool> = group.kind.group_members().iter().map(|e| matches!(&e.kind, ElementKind::Shape(s) if s.merge)).collect();
    assert_eq!(inside, [true, true]);
}

#[test]
fn group_members_keep_their_own_placement() {
    // A 50% instance in a 50% group stays 50% (#411).
    let document = r##"<DOMDocument>
      <symbols><Include href="Target.xml"/></symbols>
      <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Layer 1"><frames><DOMFrame index="0"><elements>
        <DOMGroup><matrix><Matrix a="0.5" d="0.5" tx="50" ty="50"/></matrix><members>
          <DOMSymbolInstance libraryItemName="Target"><matrix><Matrix a="0.5" d="0.5" tx="50" ty="50"/></matrix></DOMSymbolInstance>
        </members></DOMGroup>
        <DOMGroup><matrix><Matrix a="0" d="0"/></matrix><members>
          <DOMSymbolInstance libraryItemName="Target"><matrix><Matrix tx="7" ty="8"/></matrix></DOMSymbolInstance>
        </members></DOMGroup>
      </elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timelines>
    </DOMDocument>"##;
    let symbol = r#"<DOMSymbolItem name="Target"><timeline><DOMTimeline name="Target"/></timeline></DOMSymbolItem>"#;
    let (doc, _) = import(&HashMap::from([("DOMDocument.xml", document), ("LIBRARY/Target.xml", symbol)])).unwrap();
    let placed: Vec<[f32; 6]> =
        doc.timeline(0).layers[0].keyframes[0].elements.iter().map(|g| g.transform.mul(&g.kind.group_members()[0].transform).m).collect();
    // The second group's matrix can't be inverted: it frames nothing and its member stays put.
    assert_eq!(placed, [[0.5, 0.0, 0.0, 0.5, 50.0, 50.0], [1.0, 0.0, 0.0, 1.0, 7.0, 8.0]]);
}

#[test]
fn stroke_caps_joints_dashes_and_dots_come_across() {
    use anicraft_doc::{Cap, Dash, Join, StrokeStyle};
    let read = |xml: &str| {
        let el = crate::xml::parse(xml.as_bytes()).unwrap();
        let mut report = crate::Report::default();
        let st = crate::import::stroke(&el, &HashMap::new(), &mut report).unwrap();
        (st, report)
    };
    let (st, report) = read(r##"<SolidStroke weight="4" caps="none" joints="miter" miterLimit="7"><fill><SolidColor color="#112233"/></fill></SolidStroke>"##);
    assert_eq!((st.width, st.color), (4.0, Rgba::hex(0x112233)));
    assert_eq!(st.style, StrokeStyle { cap: Cap::Butt, join: Join::Miter, miter_limit: 7.0, dash: None });
    assert!(report.skipped.is_empty());
    // Round caps and joints and a miter limit of 3 are the format's defaults.
    assert_eq!(read(r#"<SolidStroke weight="1"/>"#).0.style, StrokeStyle::default());
    assert_eq!(read(r#"<SolidStroke caps="square" joints="bevel" miterLimit="1e9"/>"#).0.style, StrokeStyle { cap: Cap::Square, join: Join::Bevel, miter_limit: 100.0, dash: None });
    let (dashed, _) = read(r#"<DashedStroke weight="2" dash1="8" dash2="2.5" caps="square"/>"#);
    assert_eq!(dashed.style, StrokeStyle { cap: Cap::Square, dash: Dash::dashed(8.0, 2.5), ..StrokeStyle::default() });
    // Animate's defaults are a 6 px dash and a 6 px gap, each written only when it differs.
    assert_eq!(read(r#"<DashedStroke weight="2"/>"#).0.style.dash, Dash::dashed(6.0, 6.0));
    assert_eq!(read(r#"<DashedStroke weight="9" dash2="9"/>"#).0.style.dash, Dash::dashed(6.0, 9.0));
    assert_eq!(read(r#"<DashedStroke dash1="10"/>"#).0.style.dash, Dash::dashed(10.0, 6.0));
    // No gap draws solid.
    assert_eq!(read(r#"<DashedStroke dash1="8" dash2="0"/>"#).0.style.dash, None);
    assert_eq!(read(r#"<DottedStroke weight="3" dotSpace="5"/>"#).0.style.dash, Dash::dotted(5.0));
    for kind in ["RaggedStroke", "StippleStroke", "HatchedStroke"] {
        let (st, report) = read(&format!("<{kind} weight=\"2\"/>"));
        assert_eq!(st.style.dash, None);
        assert_eq!(report.skipped.get("ragged, stipple and hatched strokes (drawn solid)"), Some(&1));
    }
}

/// A button whose Over and Down frames hold sounds (ADR 0057), a sound on the main timeline, and
/// sounds the package doesn't hold in a form anyone can read.
const SOUND_DOC: &str = r##"<DOMDocument width="200" height="100">
  <media>
    <DOMSoundItem name="Sounds/click.wav" href="Sounds/click.wav" soundDataHRef="M 1 1.dat"/>
    <DOMSoundItem name="private.mp3" href="private.mp3" soundDataHRef="M 2 2.dat"/>
    <DOMSoundItem name="gone.wav" href="gone.wav"/>
  </media>
  <symbols><Include href="Push.xml"/></symbols>
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Music"><frames><DOMFrame index="0" duration="4" soundName="Sounds/click.wav" soundSync="start" soundLoopMode="repeat" soundLoop="3"/></frames></DOMLayer>
    <DOMLayer name="Buttons"><frames><DOMFrame index="0" duration="4"><elements><DOMSymbolInstance libraryItemName="Push"/></elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

const PUSH: &str = r##"<DOMSymbolItem name="Push" symbolType="button"><timeline><DOMTimeline name="Push"><layers>
  <DOMLayer name="Sounds"><frames>
    <DOMFrame index="0"/>
    <DOMFrame index="1" soundName="Sounds/click.wav"/>
    <DOMFrame index="2" soundName="Sounds/click.wav" soundSync="stream" soundEffect="fade in"/>
    <DOMFrame index="3" soundName="private.mp3"/>
  </frames></DOMLayer>
  <DOMLayer name="Art"><frames><DOMFrame index="0" duration="4"><elements><DOMRectangleObject x="0" y="0" objectWidth="40" objectHeight="20"><fill><SolidColor color="#3366CC"/></fill></DOMRectangleObject></elements></DOMFrame></frames></DOMLayer>
</layers></DOMTimeline></timeline></DOMSymbolItem>"##;

#[test]
fn sounds_in_the_package_come_across_on_frames_and_buttons_states() {
    let wav = anicraft_doc::audio::wav::encode(8000, 1, &[0.25; 800], anicraft_doc::audio::wav::Bits::Int16);
    let files: HashMap<String, Vec<u8>> = HashMap::from([
        ("DOMDocument.xml".to_string(), SOUND_DOC.as_bytes().to_vec()),
        ("LIBRARY/Push.xml".to_string(), PUSH.as_bytes().to_vec()),
        // An XFL folder keeps the original file in LIBRARY/.
        ("LIBRARY/Sounds/click.wav".to_string(), wav),
        // Animate's own encoding is not public: reported, not guessed.
        ("bin/M 2 2.dat".to_string(), vec![1, 2, 3, 4, 5, 6, 7, 8]),
    ]);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = import_with(&read).unwrap();
    assert_eq!(doc.sounds.iter().map(|s| (s.name.as_str(), s.rate, s.frames)).collect::<Vec<_>>(), [("click.wav", 8000, 800)]);
    let click = doc.sounds[0].id;
    let path = doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Sound(click)));
    assert_eq!(path, "Sounds/click.wav", "in its Library folder");
    // The main timeline's: Start, three times.
    let music = doc.timeline(0).layers[0].keyframes[0].sound.clone().unwrap();
    assert_eq!((music.sound, music.sync, music.repeat, music.looping), (click, anicraft_doc::SoundSync::Start, 3, false));
    // The button's Over and Down frames keep theirs; the Hit frame's sound isn't readable.
    let push = doc.library.iter().find(|s| s.name == "Push").unwrap();
    let on = |f: u32| push.timeline.layers[0].keyframes.iter().find(|k| k.start == f).and_then(|k| k.sound.clone());
    assert_eq!(on(1).map(|s| (s.sound, s.sync)), Some((click, anicraft_doc::SoundSync::Event)));
    assert_eq!(on(2).map(|s| (s.sound, s.sync)), Some((click, anicraft_doc::SoundSync::Stream)));
    assert_eq!(on(3), None);
    assert_eq!(on(2).map(|s| s.effect), Some(anicraft_doc::sound::SoundEffect::FadeIn), "the effect comes across (ADR 0086)");
    // Sounds that don't come across are named, with their format (ADR 0086).
    assert_eq!(report.names.get("sounds in formats AniCraft can't decode"), Some(&vec!["private.mp3: Animate's own sound data".to_string()]));
    assert_eq!(report.names.get("missing sound files"), Some(&vec!["gone.wav".to_string()]));
    assert_eq!(report.skipped.get("frame sounds whose sound isn't in the package"), Some(&1));
    assert!(report.summary().contains("1 missing sound files (gone.wav)"), "{}", report.summary());
    // In the player, going over the button starts its Over sound.
    use anicraft_doc::player::{Event, NoScripts, input, start};
    let s = start(&doc, &mut NoScripts);
    let s = input(&doc, &s.state, &[Event::Move(anicraft_geom::Vec2::new(10.0, 10.0))], &mut NoScripts);
    assert_eq!(s.sounds.iter().map(|x| x.sound.sound).collect::<Vec<_>>(), [click]);
}

/// #163: Animate leaves the default stage colour (white) out of `DOMDocument.xml`, so a file
/// without `backgroundColor` has a white stage, not a black one.
#[test]
fn a_document_without_a_background_colour_has_a_white_stage() {
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> {
        Ok((p == "DOMDocument.xml").then(|| br#"<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" frameRate="12"><timelines><DOMTimeline name="Scene 1"/></timelines></DOMDocument>"#.to_vec()))
    };
    let (doc, _) = import_with(&read).unwrap();
    assert_eq!(doc.stage.background, Rgba::WHITE);
    assert_eq!((doc.stage.width, doc.stage.height), (550, 400), "Animate's default size, also left out");
}

/// ADR 0086: hostile sound data (cut short, sizes that lie, a sample count far past the file)
/// never panics, never allocates past the file, and is either read for what it holds or named
/// in the report.
#[test]
fn hostile_sound_files_are_bounded_and_named() {
    let doc = r#"<DOMDocument><media><DOMSoundItem name="s.wav" href="s.wav" sampleCount="99999999999999" format="44kHz 16bit Stereo"/></media>
      <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="L"><frames><DOMFrame index="0" soundName="s.wav" soundLoop="4294967295"/></frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#;
    let open = |wav: Vec<u8>| {
        let files: HashMap<String, Vec<u8>> = HashMap::from([("DOMDocument.xml".to_string(), doc.as_bytes().to_vec()), ("LIBRARY/s.wav".to_string(), wav)]);
        let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
        import_with(&read).unwrap()
    };
    let good = anicraft_doc::audio::wav::encode(8000, 2, &[0.5; 1600], anicraft_doc::audio::wav::Bits::Int16);
    let (d, _) = open(good.clone());
    assert_eq!(d.sounds[0].frames, 800);
    assert_eq!(d.timeline(0).layers[0].keyframes[0].sound.as_ref().unwrap().repeat, anicraft_doc::sound::MAX_REPEAT, "repeats bounded");
    // Every cut: read for what it holds, or named.
    for cut in 0..good.len() {
        let (d, r) = open(good[..cut].to_vec());
        assert!(d.sounds.len() == 1 || r.names.values().flatten().any(|n| n.starts_with("s.wav")), "cut at {cut}: {r:?}");
        assert!(d.sounds.iter().all(|s| s.frames <= 800));
    }
    // The data chunk claiming 4 GB, and the RIFF size lying: only what is there is read.
    let mut lying = good.clone();
    lying[40..44].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    lying[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let (d, _) = open(lying);
    assert_eq!(d.sounds[0].frames, 800);
    // 32 channels of one byte each claimed in a tiny file: no frames, so named, not imported.
    let mut wide = good;
    wide[22..24].copy_from_slice(&32u16.to_le_bytes());
    wide[32..34].copy_from_slice(&64u16.to_le_bytes());
    wide.truncate(44 + 10);
    let (d, r) = open(wide);
    assert!(d.sounds.is_empty() && r.skipped.get("sounds in formats AniCraft can't decode") == Some(&1), "{r:?}");
}

/// ADR 0086: the report names what didn't come across, a few per kind in the summary line.
#[test]
fn the_summary_names_a_few_items_and_counts_the_rest() {
    let mut media = String::new();
    for i in 0..5 {
        media.push_str(&format!(r#"<DOMSoundItem name="gone{i}.wav" href="gone{i}.wav"/>"#));
    }
    let doc = format!(r#"<DOMDocument><media>{media}</media><timelines><DOMTimeline name="Scene 1"/></timelines></DOMDocument>"#);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok((p == "DOMDocument.xml").then(|| doc.as_bytes().to_vec())) };
    let (_, r) = import_with(&read).unwrap();
    assert_eq!(r.names["missing sound files"].len(), 5);
    assert!(r.summary().contains("5 missing sound files (gone0.wav; gone1.wav; gone2.wav and 2 more)"), "{}", r.summary());
}

/// ADR 0086: a `.fla` whose package sits in one top folder opens; one with the package at its
/// root and a folder beside it opens the root one; several documents in top folders are
/// refused with their names.
#[test]
fn a_package_in_one_top_folder_opens_and_several_are_named() {
    use crate::zip::pack::{Raw, write};
    let doc = br#"<DOMDocument width="99"><timelines><DOMTimeline name="Scene 1"/></timelines></DOMDocument>"#;
    let other = br#"<DOMDocument width="42"><timelines><DOMTimeline name="Scene 1"/></timelines></DOMDocument>"#;
    let fla = |entries: &[(&str, &[u8])]| write(&entries.iter().map(|(n, b)| Raw::new(n, b, true)).collect::<Vec<_>>(), false);
    let (d, _) = crate::import_fla(&fla(&[("Walk/DOMDocument.xml", doc), ("__MACOSX/Walk/._DOMDocument.xml", b"\0\x05\x16\x07")])).unwrap();
    assert_eq!(d.stage.width, 99);
    let (d, _) = crate::import_fla(&fla(&[("DOMDocument.xml", doc), ("Old/DOMDocument.xml", other)])).unwrap();
    assert_eq!(d.stage.width, 99, "the package at the root wins");
    let e = crate::import_fla(&fla(&[("Walk/DOMDocument.xml", doc), ("Run/DOMDocument.xml", other)])).err().unwrap().to_string();
    assert!(e.contains("2 Animate documents (Run, Walk)"), "{e}");
    // Deeper than one folder isn't looked for.
    let e = crate::import_fla(&fla(&[("a/b/DOMDocument.xml", doc)])).err().unwrap().to_string();
    assert!(e.contains("no DOMDocument.xml"), "{e}");
}

#[test]
fn grouped_instance_is_not_scaled_twice() {
    // Issue #411: a group's members are written in the group's parent space; the group's own
    // matrix only describes its frame. A 200×200 box at 50% inside a group lands where the same
    // box at 50% outside one would, not at 25%.
    let document = r##"<DOMDocument width="400" height="200">
      <symbols><Include href="Box.xml"/></symbols>
      <timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Layer 1"><frames><DOMFrame index="0"><elements>
        <DOMGroup><matrix><Matrix a="0.5" d="0.5" tx="50" ty="50"/></matrix><members>
          <DOMSymbolInstance libraryItemName="Box"><matrix><Matrix a="0.5" d="0.5" tx="50" ty="50"/></matrix></DOMSymbolInstance>
        </members></DOMGroup>
        <DOMSymbolInstance libraryItemName="Box"><matrix><Matrix a="0.5" d="0.5" tx="250" ty="50"/></matrix></DOMSymbolInstance>
        <DOMGroup><matrix><Matrix tx="10" ty="20"/></matrix><members>
          <DOMGroup><matrix><Matrix a="2" d="2" tx="30" ty="40"/></matrix><members>
            <DOMSymbolInstance libraryItemName="Box"><matrix><Matrix a="0.25" d="0.25" tx="300" ty="100"/></matrix></DOMSymbolInstance>
          </members></DOMGroup>
        </members></DOMGroup>
      </elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timelines>
    </DOMDocument>"##;
    let symbol = r##"<DOMSymbolItem name="Box" symbolType="graphic"><timeline><DOMTimeline name="Box"><layers><DOMLayer name="Layer 1"><frames><DOMFrame index="0"><elements>
        <DOMRectangleObject x="0" y="0" objectWidth="200" objectHeight="200"><fill><SolidColor color="#FF0000"/></fill></DOMRectangleObject>
      </elements></DOMFrame></frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;
    let files = HashMap::from([("DOMDocument.xml", document), ("LIBRARY/Box.xml", symbol)]);
    let (doc, _) = import(&files).unwrap();
    let els = &doc.timeline(0).layers[0].keyframes[0].elements;
    let corners = |m: anicraft_geom::Affine| {
        let (a, b) = (m.apply(anicraft_geom::Vec2::new(0.0, 0.0)), m.apply(anicraft_geom::Vec2::new(200.0, 200.0)));
        [a.x, a.y, b.x, b.y].map(|v| (v * 1000.0).round() / 1000.0)
    };
    let grouped = els[0].transform.mul(&els[0].kind.group_members()[0].transform);
    assert_eq!(corners(grouped), [50.0, 50.0, 150.0, 150.0], "grouped instance");
    assert_eq!(corners(els[1].transform), [250.0, 50.0, 350.0, 150.0], "loose instance");
    let outer = &els[2];
    let inner = &outer.kind.group_members()[0];
    let nested = outer.transform.mul(&inner.transform).mul(&inner.kind.group_members()[0].transform);
    assert_eq!(corners(nested), [300.0, 100.0, 350.0, 150.0], "instance in nested groups");
    // The groups keep their own matrices for editing.
    assert_eq!(els[0].transform.m, [0.5, 0.0, 0.0, 0.5, 50.0, 50.0]);
    assert_eq!(outer.transform.m, [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]);
}

#[test]
fn folder_packages_refuse_backslash_paths_that_leave_the_package() {
    let dir = std::env::temp_dir().join(format!("anicraft-xfl-escape-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("LIBRARY")).unwrap();
    std::fs::write(dir.join("DOMDocument.xml"), r#"<DOMDocument><symbols><Include href="..\Ball.xml"/></symbols></DOMDocument>"#).unwrap();
    // On Windows this lands outside LIBRARY; elsewhere it is a file named `..\Ball.xml`.
    std::fs::write(dir.join("LIBRARY/..\\Ball.xml"), BALL).unwrap();
    let (doc, report) = crate::import_path(&dir).unwrap();
    assert!(doc.library.is_empty(), "the escaping path was read");
    assert_eq!(report.skipped.get("missing library files"), Some(&1));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn folder_import_enforces_full_path_boundaries() {
    let dir = std::env::temp_dir().join(format!("anicraft-xfl-bounds-{}", std::process::id()));
    let outside = dir.join("outside");
    let pkg = dir.join("pkg");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(pkg.join("LIBRARY")).unwrap();
    std::fs::write(outside.join("Escape.xml"), BALL).unwrap();
    std::fs::write(pkg.join("LIBRARY/Real.xml"), BALL.replace("name=\"Ball\"", "name=\"Real\"")).unwrap();
    std::fs::write(
        pkg.join("DOMDocument.xml"),
        r#"<DOMDocument><symbols>
          <Include href="../outside/Escape.xml"/>
          <Include href="..\outside\Escape.xml"/>
          <Include href="./Real.xml"/>
          <Include href="C:Escape.xml"/>
          <Include href="/Escape.xml"/>
          <Include href="Real.xml"/>
        </symbols></DOMDocument>"#,
    )
    .unwrap();
    let (doc, report) = crate::import_path(&pkg).unwrap();
    assert_eq!(doc.library.len(), 1, "only Real.xml inside LIBRARY is imported");
    assert_eq!(report.skipped.get("missing library files"), Some(&5));

    #[cfg(unix)]
    {
        let dir2 = std::env::temp_dir().join(format!("anicraft-xfl-dirsym-{}", std::process::id()));
        let ext = dir2.join("ext");
        let pkg2 = dir2.join("pkg");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::create_dir_all(&pkg2).unwrap();
        std::fs::write(ext.join("Ball.xml"), BALL).unwrap();
        std::os::unix::fs::symlink(&ext, pkg2.join("LIBRARY")).unwrap();
        std::fs::write(pkg2.join("DOMDocument.xml"), r#"<DOMDocument><symbols><Include href="Ball.xml"/></symbols></DOMDocument>"#).unwrap();
        let (doc2, report2) = crate::import_path(&pkg2).unwrap();
        assert!(doc2.library.is_empty(), "intermediate directory symlink must not be traversed");
        assert_eq!(report2.skipped.get("missing library files"), Some(&1));
        std::fs::remove_dir_all(&dir2).ok();
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bitmap_import_enforces_aggregate_pixel_budget() {
    assert_eq!(crate::import::MAX_BITMAP_PIXELS, 256 * 1024 * 1024);
    let doc_xml = r#"<DOMDocument><media>
      <DOMBitmapItem name="a.png" href="a.png"/>
      <DOMBitmapItem name="b.png" href="b.png"/>
      <DOMBitmapItem name="c.png" href="c.png"/>
    </media></DOMDocument>"#;
    let files: HashMap<String, Vec<u8>> = HashMap::from([
        ("DOMDocument.xml".to_string(), doc_xml.as_bytes().to_vec()),
        ("LIBRARY/a.png".to_string(), png_2x1()),
        ("LIBRARY/b.png".to_string(), png_2x1()),
        ("LIBRARY/c.png".to_string(), png_2x1()),
    ]);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = crate::import::import_with_bitmap_budget(&read, 4).unwrap();
    assert_eq!(doc.bitmaps.len(), 2, "two 2x1 bitmaps fit in the 4-pixel budget");
    assert_eq!(report.skipped.get("bitmaps past the import pixel budget"), Some(&1));
}

#[test]
fn valid_long_bitmap_sequence_imports_under_budget() {
    let count = 200usize;
    let mut media = String::new();
    let mut frames = String::new();
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let img = png_2x1();
    for i in 0..count {
        let name = format!("seq/frame_{i:04}.png");
        media.push_str(&format!(r#"<DOMBitmapItem name="{name}" href="{name}"/>"#));
        frames.push_str(&format!(r#"<DOMFrame index="{i}"><elements><DOMBitmapInstance libraryItemName="{name}"/></elements></DOMFrame>"#));
        files.insert(format!("LIBRARY/{name}"), img.clone());
    }
    let doc_xml = format!(
        r#"<DOMDocument><media>{media}</media><timelines><DOMTimeline name="Scene 1"><layers><DOMLayer name="Seq"><frames>{frames}</frames></DOMLayer></layers></DOMTimeline></timelines></DOMDocument>"#
    );
    files.insert("DOMDocument.xml".to_string(), doc_xml.into_bytes());
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = import_with(&read).unwrap();
    assert_eq!(doc.bitmaps.len(), count, "all {count} sequence bitmaps fit within the 256 MP budget");
    assert_eq!(doc.timeline(0).layers[0].keyframes.len(), count);
    assert!(!report.skipped.contains_key("bitmaps past the import pixel budget"));
}

/// A bitmap in Animate's own data layout (`anicraft_flamedia`), written from its grammar:
/// premultiplied A,R,G,B rows in zlib chunks.
fn animate_bitmap(width: u16, height: u16, argb: &[u8]) -> Vec<u8> {
    let mut out = vec![3, 5];
    for v in [width * 4, width, height] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for twips in [0i32, i32::from(width) * 20, 0, i32::from(height) * 20] {
        out.extend_from_slice(&twips.to_le_bytes());
    }
    out.extend_from_slice(&[1, 1]);
    for part in miniz_oxide::deflate::compress_to_vec_zlib(argb, 6).chunks(3) {
        out.extend_from_slice(&(part.len() as u16).to_le_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

const OWN_MEDIA_DOC: &str = r##"<DOMDocument width="200" height="100">
  <media>
    <DOMBitmapItem name="Copyright/badge.png" href="Copyright/badge.png" bitmapDataHRef="M 4 4.dat" frameRight="40" frameBottom="20"/>
    <DOMBitmapItem name="cut.png" href="cut.png" bitmapDataHRef="M 5 5.dat"/>
    <DOMSoundItem name="Audio/blip" href="Audio/blip.wav" soundDataHRef="M 6 6.dat" format="44kHz 16bit Stereo" sampleCount="3" dataLength="12"/>
    <DOMSoundItem name="Audio/hum" href="Audio/hum.wav" soundDataHRef="M 7 7.dat" format="22kHz 16bit Mono" sampleCount="4"/>
    <DOMSoundItem name="Audio/eight" soundDataHRef="M 8 8.dat" format="44kHz 8bit Mono" sampleCount="4"/>
    <DOMSoundItem name="Audio/other" soundDataHRef="M 9 9.dat" format="44kHz 16bit Mono" sampleCount="400"/>
  </media>
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Sound"><frames><DOMFrame index="0" duration="4" soundName="Audio/blip" soundSync="stream"/></frames></DOMLayer>
    <DOMLayer name="Art"><frames><DOMFrame index="0" duration="4"><elements><DOMBitmapInstance libraryItemName="Copyright/badge.png"/></elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

#[test]
fn bitmaps_and_sounds_in_animates_own_data_layout_come_across() {
    // Opaque red, half-transparent blue (premultiplied), clear, opaque near-black.
    let argb = [255, 255, 0, 0, 128, 0, 0, 128, 0, 0, 0, 0, 255, 0x23, 0x1f, 0x20];
    let badge = animate_bitmap(2, 2, &argb);
    let pcm = |s: &[i16]| -> Vec<u8> { s.iter().flat_map(|v| v.to_le_bytes()).collect() };
    let files: HashMap<String, Vec<u8>> = HashMap::from([
        ("DOMDocument.xml".to_string(), OWN_MEDIA_DOC.as_bytes().to_vec()),
        ("bin/M 4 4.dat".to_string(), badge.clone()),
        ("bin/M 5 5.dat".to_string(), badge[..badge.len() - 3].to_vec()),
        ("bin/M 6 6.dat".to_string(), pcm(&[0, 16_384, -16_384, 0, 32_767, -32_768])),
        ("bin/M 7 7.dat".to_string(), pcm(&[1, 2, 3, 4])),
        ("bin/M 8 8.dat".to_string(), vec![128, 129, 130, 131]),
        ("bin/M 9 9.dat".to_string(), vec![1, 2, 3, 4, 5, 6, 7, 8]),
    ]);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = import_with(&read).unwrap();

    assert_eq!(doc.bitmaps.iter().map(|b| (b.name.as_str(), b.width, b.height)).collect::<Vec<_>>(), [("badge.png", 2, 2)]);
    let px = doc.bitmaps[0].pixels().unwrap();
    assert_eq!(px.rgba, [255, 0, 0, 255, 0, 0, 255, 128, 0, 0, 0, 0, 0x23, 0x1f, 0x20, 255], "straight alpha, A,R,G,B order");
    let path = doc.entry_path(anicraft_doc::LibraryEntry::Item(anicraft_doc::LibraryItem::Bitmap(doc.bitmaps[0].id)));
    assert_eq!(path, "Copyright/badge.png");
    assert_eq!(doc.timeline(0).layers[1].keyframes[0].elements.len(), 1, "its instance is placed");
    assert_eq!(report.names.get("bitmaps in Animate's own format AniCraft can't decode"), Some(&vec!["cut.png: the data is cut short".to_string()]));

    let sounds: Vec<_> = doc.sounds.iter().map(|s| (s.name.as_str(), s.format, s.rate, s.channels, s.frames)).collect();
    use anicraft_doc::SoundFormat::Wav;
    assert_eq!(sounds, [("blip", Wav, 44_100, 2, 3), ("hum", Wav, 22_050, 1, 4)]);
    let blip = doc.sounds[0].decoded().unwrap();
    assert_eq!(blip.pcm.samples, [0.0, 0.5, -0.5, 0.0, 32_767.0 / 32_768.0, -1.0], "left, right interleaved, bit for bit");
    assert_eq!(doc.timeline(0).layers[0].keyframes[0].sound.as_ref().map(|s| s.sound), Some(doc.sounds[0].id));
    assert_eq!(
        report.names.get("sounds in formats AniCraft can't decode"),
        Some(&vec!["Audio/eight: Animate's own sound data (8-bit sound data, which AniCraft doesn't read yet)".to_string(), "Audio/other: Animate's own sound data".to_string()])
    );
}

#[test]
fn bitmaps_in_animates_own_data_share_one_pixel_budget() {
    let doc = r##"<DOMDocument width="200" height="100">
  <media>
    <DOMBitmapItem name="cut.png" bitmapDataHRef="M 1 1.dat"/>
    <DOMBitmapItem name="first.png" bitmapDataHRef="M 2 2.dat"/>
    <DOMBitmapItem name="second.png" bitmapDataHRef="M 3 3.dat"/>
    <DOMBitmapItem name="junk.png" bitmapDataHRef="M 4 4.dat"/>
    <DOMBitmapItem name="dot.png" bitmapDataHRef="M 5 5.dat"/>
  </media>
</DOMDocument>"##;
    let square = animate_bitmap(2, 2, &[255; 16]);
    // A bitmap header whose pixels are not zlib at all: it would be reported as damaged if it
    // were inflated.
    let junk: Vec<u8> = square[..26].iter().copied().chain([3, 0, 1, 2, 3, 0, 0]).collect();
    let files: HashMap<String, Vec<u8>> = HashMap::from([
        ("DOMDocument.xml".to_string(), doc.as_bytes().to_vec()),
        ("bin/M 1 1.dat".to_string(), square[..square.len() - 3].to_vec()),
        ("bin/M 2 2.dat".to_string(), square.clone()),
        ("bin/M 3 3.dat".to_string(), square.clone()),
        ("bin/M 4 4.dat".to_string(), junk),
        ("bin/M 5 5.dat".to_string(), animate_bitmap(1, 1, &[255, 1, 2, 3])),
    ]);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    const OVER: &str = "bitmaps past the import pixel budget";
    const BAD: &str = "bitmaps in Animate's own format AniCraft can't decode";

    // Room for one 2 x 2 image (4 pixels) and one 1 x 1. The cut one fails and costs
    // nothing; the second square and the junk are refused from their headers, never inflated;
    // the dot after them still fits.
    let (doc, report) = crate::import::import_with_bitmap_budget(&read, 5).unwrap();
    assert_eq!(doc.bitmaps.iter().map(|b| (b.name.as_str(), b.width, b.height)).collect::<Vec<_>>(), [("first.png", 2, 2), ("dot.png", 1, 1)]);
    assert_eq!(report.names.get(OVER), Some(&vec!["second.png: 2 × 2 px".to_string(), "junk.png: 2 × 2 px".to_string()]));
    assert_eq!(report.skipped.get(OVER), Some(&2));
    assert_eq!(report.names.get(BAD), Some(&vec!["cut.png: the data is cut short".to_string()]));

    // With the full budget all three good ones come across, and the junk is damaged: so above,
    // it was left out unread.
    let (doc, report) = import_with(&read).unwrap();
    assert_eq!(doc.bitmaps.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["first.png", "second.png", "dot.png"]);
    assert_eq!(report.skipped.get(OVER), None);
    assert_eq!(
        report.names.get(BAD),
        Some(&vec!["cut.png: the data is cut short".to_string(), "junk.png: damaged data: the pixel data doesn't inflate".to_string()])
    );
}

#[test]
fn a_wav_or_ogg_or_mp3_data_file_of_the_raw_pcm_length_is_read_as_that_file() {
    // 100 frames of 8 kHz mono WAV, whose length is also a whole number of frames of the item's
    // own 44.1 kHz stereo raw PCM, and the item's sampleCount says that many.
    let wav = anicraft_doc::audio::wav::encode(8000, 1, &[0.25; 100], anicraft_doc::audio::wav::Bits::Int16);
    assert_eq!(wav.len() % 4, 0);
    let doc = format!(
        r#"<DOMDocument width="200" height="100"><media><DOMSoundItem name="tone" soundDataHRef="M 1 1.dat" format="44kHz 16bit Stereo" sampleCount="{}"/></media></DOMDocument>"#,
        wav.len() / 4
    );
    let files: HashMap<String, Vec<u8>> = HashMap::from([("DOMDocument.xml".to_string(), doc.as_bytes().to_vec()), ("bin/M 1 1.dat".to_string(), wav)]);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = import_with(&read).unwrap();
    let sounds: Vec<_> = doc.sounds.iter().map(|s| (s.name.as_str(), s.format, s.rate, s.channels, s.frames)).collect();
    assert_eq!(sounds, [("tone", anicraft_doc::SoundFormat::Wav, 8_000, 1, 100)], "the WAV itself, not its bytes as PCM");
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn a_damaged_wav_of_the_raw_pcm_length_stays_raw_pcm() {
    // An 8 kHz mono WAV of four samples whose data chunk claims 1,000,000 bytes: its decoder
    // would read the four (it forgives the overstated length), but the file isn't whole, so it
    // is the item's 44.1 kHz PCM, a sample for every two bytes.
    let mut wav = anicraft_doc::audio::wav::encode(8000, 1, &[0.25; 4], anicraft_doc::audio::wav::Bits::Int16);
    let samples = wav.len() / 2;
    let data_at = wav.windows(4).position(|w| w == b"data").unwrap() + 4;
    wav[data_at..data_at + 4].copy_from_slice(&1_000_000u32.to_le_bytes());
    assert!(anicraft_doc::Sound::from_data(anicraft_doc::SoundId(0), "w", wav.clone()).is_ok(), "the decoder forgives it");
    let doc = format!(r#"<DOMDocument width="200" height="100"><media><DOMSoundItem name="tone" soundDataHRef="M 1 1.dat" format="44kHz 16bit Mono" sampleCount="{samples}"/></media></DOMDocument>"#);
    let files: HashMap<String, Vec<u8>> = HashMap::from([("DOMDocument.xml".to_string(), doc.into_bytes()), ("bin/M 1 1.dat".to_string(), wav)]);
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = import_with(&read).unwrap();
    let sounds: Vec<_> = doc.sounds.iter().map(|s| (s.format, s.rate, s.channels, s.frames)).collect();
    assert_eq!(sounds, [(anicraft_doc::SoundFormat::Wav, 44_100, 1, samples as u64)], "the item's PCM, not the cut-short WAV");
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn only_whole_containers_win_over_raw_pcm() {
    use crate::import::complete_container;
    use anicraft_doc::audio::Format;
    let whole = anicraft_doc::audio::wav::encode(8000, 1, &[0.25; 4], anicraft_doc::audio::wav::Bits::Int16);
    assert!(complete_container(&whole, Format::Wav));
    assert!(!complete_container(&whole[..whole.len() - 1], Format::Wav), "cut short");
    let mut no_data = whole.clone();
    let at = no_data.windows(4).position(|w| w == b"data").unwrap();
    no_data[at..at + 4].copy_from_slice(b"junk");
    assert!(!complete_container(&no_data, Format::Wav), "no data chunk");
    let mut riff_over = whole.clone();
    riff_over[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(!complete_container(&riff_over, Format::Wav), "a RIFF size past the file");
    // Ogg pages (header, lacing, body), the container only: whole when they run exactly to the
    // end and the last one ends the stream.
    let page = |flags: u8, body: &[u8]| -> Vec<u8> {
        let mut p = b"OggS".to_vec();
        p.extend_from_slice(&[0, flags]);
        p.extend_from_slice(&[0; 20]);
        p.push(1);
        p.push(body.len() as u8);
        p.extend_from_slice(body);
        p
    };
    let stream = [page(0x02, b"head"), page(0x04, b"tail")].concat();
    assert!(complete_container(&stream, Format::Vorbis));
    assert!(!complete_container(&stream[..stream.len() - 1], Format::Vorbis), "cut short");
    assert!(!complete_container(&[page(0x02, b"head"), page(0x00, b"more")].concat(), Format::Vorbis), "never ends the stream");
    assert!(!complete_container(&[stream.clone(), b"xx".to_vec()].concat(), Format::Vorbis), "bytes after the last page");
}

#[test]
fn raw_pcm_that_starts_like_a_sound_file_stays_raw_pcm() {
    // Raw PCM whose first samples spell a WAV, Ogg, ID3 or MPEG frame signature: the decoders
    // don't read it as such a file, so it is the item's PCM.
    let heads: [&[u8]; 4] = [b"RIFF\x24\0\0\0WAVEfmt ", b"OggS\0\x02\0\0\0\0\0\0\0\0\0\0", b"ID3\x04\0\0\0\0\0\0\0\0\0\0\0\0", &[0xFF, 0xFB, 0x90, 0x64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]];
    let mut media = String::new();
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    for (n, head) in heads.iter().enumerate() {
        assert!(anicraft_doc::audio::Format::sniff(head).is_some(), "{head:?} starts like a sound file");
        media.push_str(&format!(r#"<DOMSoundItem name="s{n}" soundDataHRef="M {n} {n}.dat" format="44kHz 16bit Mono" sampleCount="8"/>"#));
        files.insert(format!("bin/M {n} {n}.dat"), head.to_vec());
    }
    let doc = format!(r#"<DOMDocument width="200" height="100"><media>{media}</media></DOMDocument>"#);
    files.insert("DOMDocument.xml".to_string(), doc.into_bytes());
    let read = |p: &str| -> Result<Option<Vec<u8>>, XflError> { Ok(files.get(p).cloned()) };
    let (doc, report) = import_with(&read).unwrap();
    assert!(report.skipped.is_empty(), "{:?}", report.names);
    assert_eq!(doc.sounds.len(), heads.len());
    for (sound, head) in doc.sounds.iter().zip(heads) {
        assert_eq!((sound.format, sound.rate, sound.channels, sound.frames), (anicraft_doc::SoundFormat::Wav, 44_100, 1, 8), "{}", sound.name);
        let want: Vec<f32> = head.as_chunks::<2>().0.iter().map(|s| f32::from(i16::from_le_bytes(*s)) / 32_768.0).collect();
        assert_eq!(sound.decoded().unwrap().pcm.samples, want, "{}: bit for bit", sound.name);
    }
}
