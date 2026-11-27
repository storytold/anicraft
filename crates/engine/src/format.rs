//! The `.anicraft` native format: a versioned JSON envelope around the document.

use crate::EngineError;
use anicraft_doc::{Document, path_table};
use anicraft_geom::Path;
use craft_anicraft_format::PayloadDecoder;
pub use craft_anicraft_format::{FORMAT_NAME, MAX_FILE_BYTES};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The newest envelope version this build reads (v1 inline paths; v2 shared path table; v3
/// named scene timelines; v4 mixed armatures; v5 extended motion paths).
pub const FORMAT_VERSION: u32 = EXTENDED_MOTION_PATH_VERSION;
/// The version a document is written as unless it needs a later one: the shared envelope's.
/// Builds from before ADR 0098 read up to this.
pub const BASE_VERSION: u32 = craft_anicraft_format::FORMAT_VERSION;
/// The first version that may hold mixed armatures.
pub const MIXED_ARMATURES_VERSION: u32 = 4;
/// The first version that reads explicit path timing and exact affine path evaluation.
const EXTENDED_MOTION_PATH_VERSION: u32 = 5;

/// The version `doc` is written as: the lowest that can say everything in it, so a document
/// that uses nothing new still opens in earlier builds, and one that does is refused by them
/// with a message naming the version instead of opening with parts missing.
pub fn version_for(doc: &Document) -> u32 {
    if doc.timelines().flat_map(|tl| &tl.layers).flat_map(|layer| &layer.keyframes)
        .filter_map(|key| key.motion.as_ref()).any(anicraft_doc::MotionTween::has_extended_path_options)
    {
        EXTENDED_MOTION_PATH_VERSION
    } else if doc.has_mixed_armatures() {
        MIXED_ARMATURES_VERSION
    } else {
        BASE_VERSION
    }
}

/// AniCraft document deserialization uses its own path-table scope while the shared format crate
/// controls when a payload is decoded in the envelope stream.
struct DocumentDecoder;

impl PayloadDecoder<Arc<Path>> for DocumentDecoder {
    type Document = Document;

    fn decode<'de, D>(&mut self, deserializer: D, paths: Option<Vec<Arc<Path>>>) -> Result<Self::Document, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let _scope = paths.map(path_table::begin_read);
        Document::deserialize(deserializer)
    }
}

/// Whether `e` has a transform JSON cannot hold (it would write `null`, which no longer loads).
fn bad_transform(e: &Arc<anicraft_doc::Element>) -> bool {
    !e.transform.is_finite()
}

/// Replaces non-finite element transforms with identity, so a ruined transform does not
/// collapse the element to a point. Returns `None`, without copying, when nothing needs repair;
/// otherwise only the scenes, items, layers, keyframes and elements that contain one are cloned.
fn repaired_for_save(doc: &Document) -> Option<Document> {
    let layer_bad = |l: &Arc<anicraft_doc::Layer>| l.keyframes.iter().any(|k| k.elements.iter().any(bad_transform));
    let tl_bad = |t: &anicraft_doc::Timeline| t.layers.iter().any(layer_bad);
    if !doc.timelines().any(tl_bad) {
        return None;
    }
    let fix = |t: &mut anicraft_doc::Timeline| {
        for l in t.layers.iter_mut().filter(|l| layer_bad(l)) {
            for k in Arc::make_mut(l).keyframes.iter_mut().filter(|k| k.elements.iter().any(bad_transform)) {
                for e in Arc::make_mut(k).elements.iter_mut().filter(|e| bad_transform(e)) {
                    Arc::make_mut(e).transform = anicraft_geom::Affine::IDENTITY;
                }
            }
        }
    };
    let mut d = doc.clone();
    for s in d.scenes.iter_mut().filter(|s| tl_bad(&s.timeline)) {
        fix(&mut Arc::make_mut(s).timeline);
    }
    for s in d.library.iter_mut().filter(|s| tl_bad(&s.timeline)) {
        fix(&mut Arc::make_mut(s).timeline);
    }
    Some(d)
}

/// Serializer adapter that writes a non-finite `f32`/`f64` as `0.0` instead of `null`, so one
/// bad number anywhere in the document (a colour, a width, a gradient stop) cannot make the
/// saved file unloadable. `0.0` is a safe stand-in because loading repairs the fields where it
/// would not be valid (`Document::sanitize` resets fps and clamps colours, sizes and camera
/// zoom); element transforms, where zero would collapse the element, are repaired to identity
/// first by `repaired_for_save`.
struct Finite<S>(S);

/// A value serialized through [`Finite`].
struct Val<'a, T: ?Sized>(&'a T);

impl<T: ?Sized + Serialize> Serialize for Val<'_, T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(Finite(s))
    }
}

macro_rules! finite_prims {
    ($($f:ident($t:ty)),*) => {$(
        fn $f(self, v: $t) -> Result<Self::Ok, Self::Error> {
            self.0.$f(v)
        }
    )*};
}

impl<S: serde::Serializer> serde::Serializer for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Finite<S::SerializeSeq>;
    type SerializeTuple = Finite<S::SerializeTuple>;
    type SerializeTupleStruct = Finite<S::SerializeTupleStruct>;
    type SerializeTupleVariant = Finite<S::SerializeTupleVariant>;
    type SerializeMap = Finite<S::SerializeMap>;
    type SerializeStruct = Finite<S::SerializeStruct>;
    type SerializeStructVariant = Finite<S::SerializeStructVariant>;

    finite_prims!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8])
    );

    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_f32(if v.is_finite() { v } else { 0.0 })
    }
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_f64(if v.is_finite() { v } else { 0.0 })
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_none()
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_some(&Val(v))
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit()
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_struct(name)
    }
    fn serialize_unit_variant(self, name: &'static str, idx: u32, variant: &'static str) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_variant(name, idx, variant)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, name: &'static str, v: &T) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_newtype_struct(name, &Val(v))
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        idx: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_newtype_variant(name, idx, variant, &Val(v))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.0.serialize_seq(len).map(Finite)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.0.serialize_tuple(len).map(Finite)
    }
    fn serialize_tuple_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.0.serialize_tuple_struct(name, len).map(Finite)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        idx: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.0.serialize_tuple_variant(name, idx, variant, len).map(Finite)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.0.serialize_map(len).map(Finite)
    }
    fn serialize_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeStruct, Self::Error> {
        self.0.serialize_struct(name, len).map(Finite)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        idx: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.0.serialize_struct_variant(name, idx, variant, len).map(Finite)
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

impl<S: serde::ser::SerializeSeq> serde::ser::SerializeSeq for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_element(&Val(v))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}
impl<S: serde::ser::SerializeTuple> serde::ser::SerializeTuple for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_element(&Val(v))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}
impl<S: serde::ser::SerializeTupleStruct> serde::ser::SerializeTupleStruct for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(&Val(v))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}
impl<S: serde::ser::SerializeTupleVariant> serde::ser::SerializeTupleVariant for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(&Val(v))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}
impl<S: serde::ser::SerializeMap> serde::ser::SerializeMap for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> Result<(), Self::Error> {
        self.0.serialize_key(&Val(k))
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_value(&Val(v))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}
impl<S: serde::ser::SerializeStruct> serde::ser::SerializeStruct for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(key, &Val(v))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}
impl<S: serde::ser::SerializeStructVariant> serde::ser::SerializeStructVariant for Finite<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(key, &Val(v))
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        self.0.skip_field(key)
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

pub fn to_json(doc: &Document) -> Result<String, EngineError> {
    let repaired = repaired_for_save(doc);
    let doc = repaired.as_ref().unwrap_or(doc);
    let paths = path_table::build_table(doc);
    let generator = format!("AniCraft {}", env!("CARGO_PKG_VERSION"));
    let version = version_for(doc);
    path_table::write_with_paths(&paths, || craft_anicraft_format::write_version(version, &generator, &paths, &Val(doc))).map_err(|e| EngineError::Format(e.to_string()))
}

pub fn from_json(text: &str) -> Result<Document, EngineError> {
    let env = craft_anicraft_format::read::<Arc<Path>, _>(text, &mut DocumentDecoder)
        .map_err(|e| EngineError::Format(format!("not a valid .anicraft file: {e}")))?;
    if env.format != FORMAT_NAME {
        return Err(EngineError::Format(format!("unexpected format '{}'", env.format)));
    }
    if env.version > FORMAT_VERSION {
        return Err(EngineError::Format(format!("file version {} is newer than this AniCraft (supports {FORMAT_VERSION}): open it with the AniCraft that saved it, or a newer one", env.version)));
    }
    let mut doc = env.document.ok_or_else(|| EngineError::Format("the file has no document".into()))?;
    doc.sanitize();
    Ok(doc)
}

pub fn load(path: &std::path::Path) -> Result<Document, EngineError> {
    let len = crate::vfs::len(path)?;
    if len > MAX_FILE_BYTES {
        return Err(EngineError::Format(format!("file is {len} bytes; the limit is {MAX_FILE_BYTES}")));
    }
    from_json(&crate::vfs::read_to_string(path)?)
}

pub fn save(doc: &Document, path: &std::path::Path) -> Result<(), EngineError> {
    let json = to_json(doc)?;
    // Write next to the target, then rename: a crash mid-write never corrupts the old file.
    // The temp name is unguessable and created exclusively, so a planted file or symlink can't redirect the write.
    let tmp = path.with_extension(format!("anicraft.{}.tmp", crate::vfs::random_suffix()));
    crate::vfs::write_new(&tmp, json)?;
    if let Err(e) = crate::vfs::rename(&tmp, path) {
        let _ = crate::vfs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_doc::{Easing, Symbol, SymbolId, evaluate_layer_elements, sample};
    use serde_json::json;

    #[test]
    fn non_finite_element_transform_still_reopens() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut d = sample::demo();
            let l = Arc::make_mut(&mut Arc::make_mut(&mut d.scenes[0]).timeline.layers[0]);
            let k = Arc::make_mut(&mut l.keyframes[0]);
            Arc::make_mut(&mut k.elements[0]).transform.m[4] = bad;
            let back = from_json(&to_json(&d).unwrap()).unwrap();
            let e = &back.timeline(0).layers[0].keyframes[0].elements[0];
            assert_eq!(e.transform, anicraft_geom::Affine::IDENTITY);
        }
    }

    #[test]
    fn non_finite_stage_fps_still_reopens_as_default() {
        for bad in [f32::NAN, f32::INFINITY] {
            let mut d = sample::demo();
            d.stage.fps = bad;
            let back = from_json(&to_json(&d).unwrap()).unwrap();
            assert_eq!(back.stage.fps, 24.0);
        }
    }

    #[test]
    fn non_finite_colour_still_reopens() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut d = sample::demo();
            d.stage.background.r = bad;
            let back = from_json(&to_json(&d).unwrap()).unwrap();
            assert_eq!(back.stage.background.r, 0.0);
        }
    }

    #[test]
    fn round_trip_is_lossless() {
        let d = sample::demo();
        let back = from_json(&to_json(&d).unwrap()).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn saved_motion_paths_require_version_5_only_for_the_new_fields() {
        let mut session = crate::Session::default();
        for (command, params) in [
            ("draw.rect", json!({"x": 0, "y": 0, "w": 10, "h": 10})),
            ("modify.transform", json!({"a": 1, "b": 0, "c": 60, "d": 1, "e": 0, "f": 0})),
            ("timeline.create_motion_tween", json!({})),
            ("motion.set_path", json!({"d": "M5 5 L105 5"})),
        ] {
            session.execute(command, &params).unwrap();
        }
        let pose = |layer: &anicraft_doc::Layer, frame| evaluate_layer_elements(layer, frame).unwrap().1[0].transform;
        let legacy = pose(&session.doc.timeline(0).layers[0], 12);

        for (timing, exact_affine) in [(false, false), (true, false), (false, true), (true, true)] {
            for in_library in [false, true] {
                let mut doc = session.doc.clone();
                let layer = Arc::make_mut(&mut doc.timeline_mut(0).layers[0]);
                let key = Arc::make_mut(&mut layer.keyframes[0]);
                let source = key.elements[0].transform;
                let motion = key.motion.as_mut().unwrap();
                if timing {
                    assert!(motion.set_path_timing(vec![(0.0, Easing::QuadIn)], 0.0));
                }
                if exact_affine {
                    motion.preserve_path_affine();
                }
                let authored = motion.clone();
                let edited = pose(layer, 12);
                if timing {
                    assert!(edited.translation().x < legacy.translation().x, "the authored ease slows the first half");
                }
                if exact_affine {
                    assert_eq!(&edited.m[..4], &source.m[..4], "the authored shear survives evaluation");
                    assert_ne!(&edited.m[..4], &legacy.m[..4]);
                }
                if in_library {
                    // The only tween is now in this unused symbol.
                    let timeline = std::mem::take(doc.timeline_mut(0));
                    let id = SymbolId(doc.alloc_id());
                    doc.library.push(Arc::new(Symbol {
                        id, name: "Path fixture".into(), kind: Default::default(),
                        linkage: None, scale_grid: None, scale_grid_kept: None, timeline,
                    }));
                }

                let text = to_json(&doc).unwrap();
                let wire: serde_json::Value = serde_json::from_str(&text).unwrap();
                let expected = if timing || exact_affine { 5 } else { 3 };
                assert_eq!(wire["version"], expected, "timing={timing}, exact_affine={exact_affine}, in_library={in_library}");
                let container = if in_library { &wire["document"]["library"][0] } else { &wire["document"]["scenes"][0] };
                let saved = &container["timeline"]["layers"][0]["keyframes"][0]["motion"];
                assert_eq!(saved.get("path_timing").is_some(), timing);
                assert_eq!(saved.get("exact_path_affine").is_some(), exact_affine);

                let reopened = from_json(&text).unwrap();
                let timeline = if in_library { &reopened.library[0].timeline } else { reopened.timeline(0) };
                assert_eq!(timeline.layers[0].keyframes[0].motion.as_ref(), Some(&authored));
                assert_eq!(pose(&timeline.layers[0], 12), edited);
            }
        }
    }

    #[test]
    fn shared_paths_are_written_once_and_shared_again_on_load() {
        let d = anicraft_doc::sample::stress(2, 40, 48); // 4 keyframes per element share a path
        let json = to_json(&d).unwrap();
        let env: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(env["version"], 3);
        assert_eq!(env["paths"].as_array().unwrap().len(), 80, "one entry per distinct path");
        let back = from_json(&json).unwrap();
        assert_eq!(d, back);
        let l = &back.timeline(0).layers[0];
        let (anicraft_doc::ElementKind::Shape(a), anicraft_doc::ElementKind::Shape(b)) =
            (&l.keyframes[0].elements[0].kind, &l.keyframes[3].elements[0].kind)
        else {
            panic!()
        };
        assert!(Arc::ptr_eq(&a.path, &b.path), "sharing restored on load");
    }

    #[test]
    fn duplicate_envelope_keys_keep_last_values_and_path_table_scope() {
        let doc = sample::stress(2, 40, 48);
        let wire: serde_json::Value = serde_json::from_str(&to_json(&doc).unwrap()).unwrap();
        let paths = wire["paths"].as_array().unwrap();
        let mut earlier_paths = paths.clone();
        earlier_paths.reverse();
        assert_ne!(earlier_paths.as_slice(), paths.as_slice());
        let json = format!(
            "{{\"format\":\"other\",\"format\":\"anicraft\",\"version\":99,\"version\":3,\"paths\":{},\"paths\":{},\"document\":{},\"ignored\":true}}",
            serde_json::to_string(&earlier_paths).unwrap(),
            serde_json::to_string(paths).unwrap(),
            serde_json::to_string(&wire["document"]).unwrap(),
        );
        assert_eq!(from_json(&json).unwrap(), doc);
    }

    #[test]
    fn malformed_earlier_duplicate_document_keeps_error_precedence_and_location() {
        let doc = sample::demo();
        let wire: serde_json::Value = serde_json::from_str(&to_json(&doc).unwrap()).unwrap();
        let later_document = serde_json::to_string(&wire["document"]).unwrap();
        let json = format!("{{\n  \"format\":\"anicraft\",\n  \"version\":3,\n  \"document\":null,\n  \"document\":{later_document}\n}}");
        let error = from_json(&json).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(error.contains("not a valid .anicraft file: invalid type: null"), "{error}");
        assert!(error.contains("line 4"), "payload diagnostics keep source locations: {error}");
    }

    #[test]
    fn absent_document_keeps_the_native_format_error() {
        let error = from_json(r#"{"format":"anicraft","version":3}"#).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(error.contains("the file has no document"), "{error}");
    }

    #[test]
    fn version_1_files_still_load() {
        let v1 = r#"{"format":"anicraft","version":1,"document":{"timeline":{"layers":[{"id":1,"name":"L","keyframes":[{"start":0,"duration":2,"elements":[
            {"id":2,"kind":{"Shape":{"path":{"cmds":[{"MoveTo":{"x":0.0,"y":0.0}},{"LineTo":{"x":10.0,"y":0.0}},{"LineTo":{"x":10.0,"y":10.0}},"Close"]},"fill":{"Solid":{"r":1.0,"g":0.0,"b":0.0,"a":1.0}}}}}]}]}]},"next_id":3}}"#;
        let d = from_json(v1).unwrap();
        assert_eq!(d.frame_count(), 2);
        assert_eq!(anicraft_doc::evaluate(&d, 1).items.len(), 1);
    }

    #[test]
    fn bad_path_references_are_errors() {
        let bad = r#"{"format":"anicraft","version":2,"paths":["M0 0L1 1Z"],"document":{"timeline":{"layers":[{"id":1,"name":"L","keyframes":[{"start":0,"duration":1,"elements":[
            {"id":2,"kind":{"Shape":{"path":7}}}]}]}]}}}"#;
        assert!(from_json(bad).is_err());
        assert!(from_json(r#"{"format":"anicraft","version":2,"paths":["M0 0 Q"],"document":{}}"#).is_err());
    }

    #[test]
    fn rejects_garbage_and_future_versions() {
        assert!(from_json("{").is_err());
        assert!(from_json(r#"{"format":"other","version":1,"document":{}}"#).is_err());
        assert!(from_json(r#"{"format":"anicraft","version":99,"document":{}}"#).is_err());
    }

    #[test]
    fn missing_fields_default() {
        let d = from_json(
            r#"{"format":"anicraft","version":1,"document":{"stage":{"width":0,"height":50,"fps":-3,"background":{"r":2,"g":0,"b":0,"a":1}}}}"#,
        )
        .unwrap();
        assert_eq!(d.stage.width, 1);
        assert_eq!(d.stage.fps, 24.0);
        assert_eq!(d.stage.background.r, 1.0);
    }
}
