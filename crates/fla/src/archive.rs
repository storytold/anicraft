//! The object graph of a legacy item stream: a page's or a symbol's timeline as Flash 8–CS4
//! serialized it (ADR 0096).
//!
//! Learned differentially from the corpus's legacy files and their Animate twins; no Flash or
//! Animate code involved. An item stream (`P <n> <stamp>`, `S <n> <stamp>`, `Symbol <n>`) is one
//! byte (`01`), then an object graph written the way MFC's `CArchive` writes objects (its
//! documented behaviour): every object starts with a tag — `FF FF`, a u16 schema, a u16 length
//! and the class name for a class not seen yet; `0x8000 | n` for an object of class number `n`;
//! a bare `n` for an object written before (a back-reference); `00 00` for none; `7F FF` and a
//! u32 for numbers past 0x7FFE. Classes and objects share one numbering, from 1, in the order
//! they first appear.
//!
//! Every `CPic*` object begins alike — `04`, a flag byte, its child objects up to a `00 00`,
//! two i32 (a point in twips, or `0x80000000` for none), a u16 and a kind byte (page 07, layer
//! 0B, frame and shape 03, symbol instance 13, text 0D, bitmap 02) — and then its own fields:
//!
//! ```text
//! page      children: layers, bottom first · u16 u16 u32 · u32 n · n × 8 (ruler guides)
//! layer     children: keyframes · name · current, hidden, locked u8 · i32 · RGBA colour ·
//!           u32 outline · u32 · u8 type (0 normal, 1 guide, 3 folder, 4 mask) ·
//!           parent layer (an object: the folder, mask or guide it sits under) · u8 u8
//! frame     children: the keyframe's objects · its raw artwork (a shape body, see
//!           [`crate::shape`]) · u32 n · n × 32 (cubic drawing hints) · 18 · u16 duration ·
//!           u8 tween (0 none, 1 classic, 2 shape) · u8 tween flags · i16 ease ·
//!           u16 sound (a `Contents` media item id, 0 none) · u16 n · n × (u32 mark44, u16
//!           left, u16 right) (envelope) · u16 loops · u8 sync · u32 in · u32 out (0x3FFFFFFF
//!           unset) · u16 zoom · label · actions (16 bytes, then the frame script string) ·
//!           8 bytes · u32 label kind (1 comment) · shape-tween cache (an object) · 4 bytes ·
//!           shape hints (an object: a `CObList` of `CMorphHintItem`s, each i32 start x, y,
//!           end x, y in twips and a u32 key) · string · 5 × u32, the last 1 when custom eases
//!           follow: 6 curves (position, rotation, scale, colour, filters, all), each u32 n
//!           and n anchors of 3 points (in handle, anchor, out handle) as f64 x, y
//! shape     children: a group's members · a shape body · cubic hints; the base u16 is 1 for a
//!           drawing object; without members, flag bit 0 marks a loose (raw) shape and no
//!           bit a group of one
//! instance  a, b, c, d (16.16), tx, ty (twips) · u16 first frame · u16 loop (0 loop,
//!           1 play once, 2 single frame) · u8 · 4 × (i16 multiplier in 1/256, i16 offset)
//!           for A, R, G, B · u8 colour style (0 none, 1 brightness, 2 tint, 3 advanced,
//!           4 alpha) · u8 · i16 percent · RGBA tint · instance name · u16 library item id ·
//!           3 bytes · persistent data (u8 flag; u32 1, u32 n, n × (u8 5, name, value)) · u8 ·
//!           u8 filter count · u8 blend (SWF numbering) · u16; movie clips (`CPicSprite`) and
//!           buttons (`CPicButton`) carry more after it:
//! button    0B · actions (16 bytes, then the handler script string) · u8 (track as menu, a
//!           hypothesis: 0 throughout the corpus) · instance name · u32 0
//! sprite    08 · actions (16 bytes, then the clip-event script string) · instance name ·
//!           component settings (not decoded, skipped)
//! text      see [`crate::text`]
//! bitmap    a, b, c, d (16.16), tx, ty (twips) · u16 media item id (`Contents`) · u8 (0)
//! ```
//!
//! The instance's two base i32 are its transformation point already placed on the stage
//! (`M · p`). Strings are UTF-16 behind `FF FE FF` and a u8 length (`FF`, then a u16, past 254).
//!
//! What isn't decoded — a movie clip's component settings, filter lists, objects of other
//! classes, a text or bitmap record in a layout not seen — is skipped by finding where the
//! parent continues: its own end (`00 00`, then its base with its kind
//! byte) or the next sibling object, checked by parsing that sibling (or a reference to an
//! object read before). Class declarations in the skipped bytes are numbered as they pass; an
//! object of a known class there that doesn't parse stops the skip, since what it holds took
//! numbers that can't be counted. Nothing here panics or reads past the end; a stream that
//! doesn't make sense is refused as a whole.

use crate::shape::{self, ShapeRecord};
use crate::text::TextRecord;
use anicraft_geom::Affine;

/// Most objects one stream may hold.
pub const MAX_OBJECTS: usize = 1 << 20;
/// Deepest nesting of objects (groups in groups, a layer written inside another's record).
const MAX_DEPTH: usize = 64;
/// Longest string read, in UTF-16 units.
const MAX_STRING: usize = 1 << 20;
/// Bytes scanned while skipping undecoded objects, per stream.
const SCAN_BUDGET: usize = 256 << 20;
/// Trial parses while skipping, per stream.
const MAX_TRIALS: usize = 1 << 16;
/// Most envelope points, persistent-data entries, guides or ease anchors in one list.
const MAX_LIST: usize = 1 << 16;
/// Most references in a row looked past to find where a parent continues.
const MAX_REF_RUN: usize = 64;
/// The base point of a page, layer or frame: none.
const NO_POINT: [u8; 8] = [0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x80];

pub const KIND_PAGE: u8 = 0x07;
pub const KIND_LAYER: u8 = 0x0B;
pub const KIND_SHAPE: u8 = 0x03;
pub const KIND_INSTANCE: u8 = 0x13;
pub const KIND_BITMAP: u8 = 0x02;
/// A frame's sound out point when none is set (the sound plays to its end).
pub const NO_OUT_POINT: u32 = 0x3FFF_FFFF;
/// The first byte of a button instance's own settings, and of a movie clip's.
const BUTTON_EXTRAS: u8 = 0x0B;
const SPRITE_EXTRAS: u8 = 0x08;

/// The stream doesn't follow the grammar here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bad;

type R<T> = Result<T, Bad>;

/// A layer record.
#[derive(Clone, Debug, Default)]
pub struct LayerRec {
    pub name: String,
    pub current: bool,
    pub hidden: bool,
    pub locked: bool,
    pub outline: bool,
    pub color: [u8; 4],
    /// 0 normal, 1 guide, 3 folder, 4 mask.
    pub kind: u8,
    /// The layer it sits under (a folder, a mask or a guide), as a node.
    pub parent: Option<usize>,
    /// Its keyframes in order, as nodes.
    pub frames: Vec<usize>,
}

/// A shape hint as stored: its start and its end point, in twips.
pub type HintRec = ((i32, i32), (i32, i32));

/// A keyframe record.
#[derive(Clone, Debug, Default)]
pub struct FrameRec {
    pub duration: u16,
    /// 0 none, 1 classic (motion), 2 shape.
    pub tween: u8,
    pub tween_flags: u8,
    /// The classic ease, −100..=100.
    pub ease: i16,
    /// The sound's library item id (0: none).
    pub sound: u16,
    /// The sound's volume envelope: (time in samples at 44.1 kHz, left, right level 0..=32768).
    pub envelope: Vec<(u32, u16, u16)>,
    /// Times the sound plays (1 = once).
    pub loops: u16,
    /// The sound's sync byte (3 stream on every corpus sound frame; 0 event, 1 start, 2 stop
    /// by Flash's menu order, a hypothesis).
    pub sync: u8,
    /// The sound's in and out points in samples at 44.1 kHz ([`NO_OUT_POINT`]: none).
    pub in_point: u32,
    pub out_point: u32,
    pub label: String,
    /// 1: the label is a comment (0, a name, and 2, an anchor, are hypotheses: the corpus
    /// labels are all comments).
    pub label_kind: u32,
    /// Custom ease curves by group (position, rotation, scale, colour, filters, all): each a
    /// list of anchors (in handle x, y, anchor x, y, out handle x, y). Empty: no custom ease.
    pub eases: Vec<Vec<[f64; 6]>>,
    /// The keyframe's raw artwork (`None` when it didn't decode).
    pub raw: Option<ShapeRecord>,
    /// Why the raw artwork didn't decode, when it didn't.
    pub raw_error: Option<shape::ShapeError>,
    /// The shape hints listed on the keyframe, in letter order: (start, end) in twips.
    pub hints: Vec<HintRec>,
    /// The frame script, as written (empty: none).
    pub script: String,
    /// The keyframe's objects, bottom first, as nodes.
    pub elements: Vec<usize>,
}

/// A symbol instance's colour effect as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorRec {
    /// Multipliers in 1/256 and offsets, alpha, red, green, blue.
    pub mul: [i16; 4],
    pub add: [i16; 4],
    /// 0 none, 1 brightness, 2 tint, 3 advanced, 4 alpha.
    pub style: u8,
    pub percent: i16,
    pub tint: [u8; 4],
}

/// Which class an instance was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InstanceClass {
    #[default]
    Graphic,
    MovieClip,
    Button,
}

/// A symbol instance record.
#[derive(Clone, Debug, Default)]
pub struct InstanceRec {
    pub class: InstanceClass,
    pub locked: bool,
    pub matrix: Affine,
    /// The transformation point placed on the stage (`M · p`), in twips.
    pub point: (i32, i32),
    pub first_frame: u16,
    pub loop_mode: u16,
    pub color: ColorRec,
    pub name: String,
    /// The library item id (`Contents`).
    pub item: u16,
    pub blend: u8,
    pub filters: u8,
    /// A button's handler or a movie clip's clip-event script, as written (empty: none).
    pub script: String,
    /// A button's track-as-menu byte (a hypothesis: 0 throughout the corpus).
    pub track_as_menu: bool,
    /// The record's tail (a movie clip's component settings, filters) was skipped.
    pub extras_skipped: bool,
}

/// A bitmap instance record.
#[derive(Clone, Debug)]
pub struct BitmapRec {
    pub matrix: Affine,
    /// The bitmap's media item id (`Contents`).
    pub item: u16,
}

/// A shape object: a drawing object, a loose shape, or a group.
#[derive(Clone, Debug, Default)]
pub struct ShapeObj {
    pub flag: u8,
    pub drawing_object: bool,
    pub record: Option<ShapeRecord>,
    pub members: Vec<usize>,
}

/// One decoded object.
#[derive(Clone, Debug)]
pub enum Node {
    /// Being read (or a class number's placeholder).
    Pending,
    Page { layers: Vec<usize> },
    Layer(LayerRec),
    Frame(Box<FrameRec>),
    Shape(ShapeObj),
    Instance(Box<InstanceRec>),
    Text(Box<TextRecord>),
    Bitmap(BitmapRec),
    /// A shape tween's cache: rebuilt by AniCraft.
    Morph,
    /// A list of objects (`CObList`, the shape-hint list).
    List(Vec<usize>),
    /// One shape hint: start and end in twips.
    Hint((i32, i32), (i32, i32)),
    /// An object not decoded, by class name.
    Skipped(String),
}

/// A stream's objects.
#[derive(Clone, Debug)]
pub struct Graph {
    pub nodes: Vec<Node>,
    /// The page.
    pub root: usize,
    /// Undecoded stretches skipped.
    pub resyncs: usize,
}

impl Graph {
    pub fn node(&self, i: usize) -> Option<&Node> {
        self.nodes.get(i)
    }
    /// The layers, bottom first, each once.
    pub fn layers(&self) -> Vec<usize> {
        let Some(Node::Page { layers }) = self.nodes.get(self.root) else { return Vec::new() };
        let mut out: Vec<usize> = Vec::with_capacity(layers.len());
        for &l in layers {
            if matches!(self.nodes.get(l), Some(Node::Layer(_))) && !out.contains(&l) {
                out.push(l);
            }
        }
        out
    }
}

#[derive(Clone, Copy)]
enum Entry {
    Null,
    Class(usize),
    Object(usize),
}

enum Tag {
    Null,
    Ref(usize),
    New(usize),
}

/// What a sibling's head starts with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Head {
    /// A new class's declaration, and an object of it.
    Declaration,
    /// An object of a class declared before.
    Object,
}

struct Parser<'a> {
    d: &'a [u8],
    at: usize,
    map: Vec<Entry>,
    classes: Vec<String>,
    nodes: Vec<Node>,
    depth: usize,
    budget: usize,
    trials: usize,
    resyncs: usize,
}

#[derive(Clone, Copy)]
struct Snap {
    at: usize,
    map: usize,
    classes: usize,
    nodes: usize,
}

struct Base {
    flag: u8,
    children: Vec<usize>,
    x: i32,
    y: i32,
    word: u16,
}

/// Decodes an item stream's object graph (its first byte, then the page object).
pub fn parse(data: &[u8]) -> Result<Graph, Bad> {
    if data.first() != Some(&0x01) {
        return Err(Bad);
    }
    let mut p = Parser { d: data, at: 1, map: vec![Entry::Null], classes: Vec::new(), nodes: Vec::new(), depth: 0, budget: SCAN_BUDGET, trials: 0, resyncs: 0 };
    let root = p.obj(None)?.ok_or(Bad)?;
    if !matches!(p.nodes.get(root), Some(Node::Page { .. })) {
        return Err(Bad);
    }
    Ok(Graph { nodes: p.nodes, root, resyncs: p.resyncs })
}

impl<'a> Parser<'a> {
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or(Bad)?;
        let s = self.d.get(self.at..end).ok_or(Bad)?;
        self.at = end;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> R<[u8; N]> {
        <[u8; N]>::try_from(self.take(N)?).map_err(|_| Bad)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.arr::<1>()?[0])
    }
    fn u16(&mut self) -> R<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    fn i16(&mut self) -> R<i16> {
        Ok(i16::from_le_bytes(self.arr()?))
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn i32(&mut self) -> R<i32> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    fn f64(&mut self) -> R<f64> {
        Ok(f64::from_le_bytes(self.arr()?))
    }
    fn skip(&mut self, n: usize) -> R<()> {
        self.take(n).map(|_| ())
    }
    /// A length-bounded list count.
    fn count(&mut self, n: u32) -> R<usize> {
        let n = n as usize;
        if n > MAX_LIST { Err(Bad) } else { Ok(n) }
    }
    /// `FF FE FF`, a u8 length (`FF`: a u16 follows; `FFFF`: a u32), UTF-16 units.
    fn string(&mut self) -> R<String> {
        if self.take(3)? != [0xFF, 0xFE, 0xFF] {
            return Err(Bad);
        }
        let mut n = self.u8()? as usize;
        if n == 0xFF {
            n = self.u16()? as usize;
            if n == 0xFFFF {
                n = self.u32()? as usize;
            }
        }
        if n > MAX_STRING {
            return Err(Bad);
        }
        let bytes = self.take(n.checked_mul(2).ok_or(Bad)?)?;
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        Ok(String::from_utf16_lossy(&units))
    }
    fn matrix(&mut self) -> R<Affine> {
        let mut m = [0f32; 6];
        for (i, v) in m.iter_mut().enumerate() {
            let raw = self.i32()? as f32;
            *v = if i < 4 { raw / 65536.0 } else { raw / 20.0 };
        }
        Ok(Affine { m })
    }

    fn snap(&self) -> Snap {
        Snap { at: self.at, map: self.map.len(), classes: self.classes.len(), nodes: self.nodes.len() }
    }
    fn restore(&mut self, s: Snap) {
        self.at = s.at;
        self.map.truncate(s.map);
        self.classes.truncate(s.classes);
        self.nodes.truncate(s.nodes);
    }

    fn class_of(&self, index: usize) -> R<usize> {
        match self.map.get(index) {
            Some(Entry::Class(c)) => Ok(*c),
            _ => Err(Bad),
        }
    }

    fn declare(&mut self, name: String) -> usize {
        let c = self.classes.len();
        self.classes.push(name);
        self.map.push(Entry::Class(c));
        c
    }

    fn tag(&mut self) -> R<Tag> {
        let t = self.u16()?;
        Ok(match t {
            0 => Tag::Null,
            0xFFFF => {
                let _schema = self.u16()?;
                let n = self.u16()? as usize;
                if !(1..=64).contains(&n) {
                    return Err(Bad);
                }
                let name = self.take(n)?;
                if !name.iter().all(|b| b.is_ascii_graphic()) {
                    return Err(Bad);
                }
                let name = String::from_utf8_lossy(name).into_owned();
                Tag::New(self.declare(name))
            }
            0x7FFF => {
                let big = self.u32()?;
                if big & 0x8000_0000 != 0 {
                    Tag::New(self.class_of((big & 0x7FFF_FFFF) as usize)?)
                } else {
                    Tag::Ref(big as usize)
                }
            }
            t if t & 0x8000 != 0 => Tag::New(self.class_of((t & 0x7FFF) as usize)?),
            t => Tag::Ref(t as usize),
        })
    }

    /// One object (`None` for the null tag). `parent` is the kind byte of the object it is a
    /// child of, when it is one: where an undecoded tail ends.
    fn obj(&mut self, parent: Option<u8>) -> R<Option<usize>> {
        match self.tag()? {
            Tag::Null => Ok(None),
            Tag::Ref(i) => match self.map.get(i) {
                Some(Entry::Object(n)) => Ok(Some(*n)),
                _ => Err(Bad),
            },
            Tag::New(class) => {
                if self.map.len() >= MAX_OBJECTS || self.depth >= MAX_DEPTH {
                    return Err(Bad);
                }
                let n = self.nodes.len();
                self.nodes.push(Node::Pending);
                self.map.push(Entry::Object(n));
                self.depth += 1;
                let node = self.body(class, parent);
                self.depth -= 1;
                let node = node?;
                if let Some(slot) = self.nodes.get_mut(n) {
                    *slot = node;
                }
                Ok(Some(n))
            }
        }
    }

    fn body(&mut self, class: usize, parent: Option<u8>) -> R<Node> {
        let name = self.classes.get(class).cloned().unwrap_or_default();
        match name.as_str() {
            "CPicPage" => {
                let base = self.base(KIND_PAGE)?;
                self.skip(8)?;
                let n = self.u32()?;
                let n = self.count(n)?;
                self.skip(n.checked_mul(8).ok_or(Bad)?)?;
                Ok(Node::Page { layers: base.children })
            }
            "CPicLayer" => self.layer(),
            "CPicFrame" => self.frame(),
            "CPicShape" => {
                let base = self.base(KIND_SHAPE)?;
                let record = self.shape_body()?;
                Ok(Node::Shape(ShapeObj { flag: base.flag, drawing_object: base.word == 1, record: Some(record), members: base.children }))
            }
            "CPicSymbol" => self.instance(InstanceClass::Graphic, parent),
            "CPicSprite" => self.instance(InstanceClass::MovieClip, parent),
            "CPicButton" => self.instance(InstanceClass::Button, parent),
            "CPicMorphShape" => self.morph(),
            "CMorphSegment" => {
                self.skip(32)?;
                let n = self.u16()?;
                for _ in 0..n {
                    self.obj(None)?;
                }
                Ok(Node::Morph)
            }
            "CMorphCurve" => {
                self.skip(36)?;
                Ok(Node::Morph)
            }
            "CObList" => {
                let n = self.u16()?;
                let mut items = Vec::with_capacity(usize::from(n).min(64));
                for _ in 0..n {
                    if let Some(i) = self.obj(None)? {
                        items.push(i);
                    }
                }
                Ok(Node::List(items))
            }
            "CMorphHintItem" => {
                let (sx, sy, ex, ey) = (self.i32()?, self.i32()?, self.i32()?, self.i32()?);
                self.skip(4)?; // the key
                Ok(Node::Hint((sx, sy), (ex, ey)))
            }
            "CPicText" => self.text(parent),
            "CPicBitmap" => self.bitmap(parent),
            // Anything newer: skipped to where its parent continues.
            _ => {
                self.resync(parent)?;
                Ok(Node::Skipped(name))
            }
        }
    }

    /// The common head of a `CPic*` object; `kind` is the kind byte it must end with.
    fn base(&mut self, kind: u8) -> R<Base> {
        if self.u8()? != 0x04 {
            return Err(Bad);
        }
        let flag = self.u8()?;
        let mut children = Vec::new();
        loop {
            let snap = self.snap();
            match self.obj(Some(kind)) {
                Ok(None) => break,
                Ok(Some(n)) => children.push(n),
                Err(Bad) => {
                    self.restore(snap);
                    children.push(self.skip_child(kind)?);
                }
            }
            if children.len() > MAX_OBJECTS {
                return Err(Bad);
            }
        }
        let x = self.i32()?;
        let y = self.i32()?;
        let word = self.u16()?;
        if self.u8()? != kind {
            return Err(Bad);
        }
        Ok(Base { flag, children, x, y, word })
    }

    /// Skips the child object starting here (one that didn't decode): its tag, then up to
    /// where its parent (of kind `parent`) continues. Returns its placeholder node.
    fn skip_child(&mut self, parent: u8) -> R<usize> {
        let class = match self.tag()? {
            Tag::New(c) => c,
            _ => return Err(Bad),
        };
        if self.map.len() >= MAX_OBJECTS {
            return Err(Bad);
        }
        let n = self.nodes.len();
        self.nodes.push(Node::Skipped(self.classes.get(class).cloned().unwrap_or_default()));
        self.map.push(Entry::Object(n));
        self.resync(Some(parent))?;
        Ok(n)
    }

    /// Moves past bytes not decoded to where the parent continues: its end (when its kind is
    /// known) or the next sibling, which must parse. Class declarations in the skipped bytes
    /// are numbered as the scan passes them, once each; a failed trial rolls back only what it
    /// read. An object of a class declared before that doesn't parse can't be skipped: it and
    /// what it holds took numbers this pass can't count, and a later back-reference would find
    /// the wrong object, so the skip fails instead.
    fn resync(&mut self, parent: Option<u8>) -> R<()> {
        let from = self.at;
        let mut numbered_to = from;
        for i in from..self.d.len() {
            self.budget = self.budget.checked_sub(1).ok_or(Bad)?;
            if parent.is_some_and(|k| self.parent_end_at(i, k)) {
                self.number_declarations(&mut numbered_to, i);
                self.at = i;
                self.resyncs += 1;
                return Ok(());
            }
            let Some(head) = self.head_at(i) else { continue };
            if self.trials < MAX_TRIALS && self.depth < MAX_DEPTH {
                self.trials += 1;
                self.number_declarations(&mut numbered_to, i);
                let numbered = self.snap();
                self.at = i;
                let ok = matches!(self.obj(parent), Ok(Some(_)));
                // What a trial reads counts against the budget too (a hostile stream can't
                // make the skipping quadratic).
                self.budget = self.budget.saturating_sub(self.at.saturating_sub(i));
                self.restore(numbered);
                if ok {
                    self.at = i;
                    self.resyncs += 1;
                    return Ok(());
                }
            }
            if head == Head::Object {
                return Err(Bad);
            }
        }
        Err(Bad)
    }

    /// Whether the parent's children end at `i`: `00 00`, then a base with no point and the
    /// parent's kind, then the start of what that kind holds next.
    fn parent_end_at(&self, i: usize, kind: u8) -> bool {
        let d = self.d;
        let at = |o: usize| i.checked_add(o).and_then(|j| d.get(j)).copied();
        if at(0) != Some(0) || at(1) != Some(0) || at(12) != Some(kind) {
            return false;
        }
        let point = i.checked_add(2).and_then(|s| d.get(s..s.checked_add(8)?));
        if point != Some(&NO_POINT[..]) || at(10).is_none_or(|w| w > 1) || at(11) != Some(0) {
            return false;
        }
        match kind {
            KIND_SHAPE => at(13 + 24) == Some(0x05),
            KIND_LAYER => at(13) == Some(0xFF) && at(14) == Some(0xFE) && at(15) == Some(0xFF),
            _ => true,
        }
    }

    /// Whether an object of a known class (or a new class's declaration) starts at `i`.
    fn sibling_head_at(&self, i: usize) -> bool {
        self.head_at(i).is_some()
    }

    /// The object of a known class (or the new class's declaration) that starts at `i`, if one
    /// does.
    fn head_at(&self, i: usize) -> Option<Head> {
        let d = self.d;
        let byte = |o: usize| i.checked_add(o).and_then(|j| d.get(j)).copied();
        let t = u16::from_le_bytes([byte(0)?, byte(1)?]);
        let (head, body) = if t == 0xFFFF {
            let len = u16::from_le_bytes([byte(4)?, byte(5)?]) as usize;
            if byte(6) != Some(b'C') || !(4..=40).contains(&len) {
                return None;
            }
            (Head::Declaration, 6 + len)
        } else if t & 0x8000 != 0 && t != 0x7FFF {
            if !matches!(self.map.get((t & 0x7FFF) as usize), Some(Entry::Class(_))) {
                return None;
            }
            (Head::Object, 2)
        } else {
            return None;
        };
        (byte(body) == Some(0x04) && byte(body + 1).is_some_and(|f| f < 0x10)).then_some(head)
    }

    /// The length of a reference at `i` (`n`, or `7F FF` and a u32) to an object read before:
    /// its number must hold an object in the map, not a class or nothing.
    fn reference_at(&self, i: usize) -> Option<usize> {
        let d = self.d;
        let byte = |o: usize| i.checked_add(o).and_then(|j| d.get(j)).copied();
        let (index, len) = match u16::from_le_bytes([byte(0)?, byte(1)?]) {
            0x7FFF => {
                let big = u32::from_le_bytes([byte(2)?, byte(3)?, byte(4)?, byte(5)?]);
                if big & 0x8000_0000 != 0 {
                    return None;
                }
                (big as usize, 6)
            }
            t if t == 0 || t & 0x8000 != 0 => return None,
            t => (usize::from(t), 2),
        };
        matches!(self.map.get(index), Some(Entry::Object(_))).then_some(len)
    }

    /// Numbers the class declarations in skipped bytes from `*at` up to `to`, and moves `*at`
    /// on, so later class numbers stay right (each declares a class and, right after, an object
    /// of it). Two declarations never overlap (a name holds no `FF`, a length no `FF FF`), so
    /// numbering a span in steps finds just what one pass over it would; a declaration that
    /// runs past `to` waits for a later step.
    fn number_declarations(&mut self, at: &mut usize, to: usize) {
        let d = self.d;
        while *at < to {
            let i = *at;
            let byte = |o: usize| i.checked_add(o).and_then(|j| d.get(j)).copied();
            let len = match (byte(4), byte(5)) {
                (Some(a), Some(b)) => u16::from_le_bytes([a, b]) as usize,
                _ => 0,
            };
            let name = i.checked_add(6).and_then(|s| d.get(s..s.checked_add(len)?));
            if byte(0) == Some(0xFF)
                && byte(1) == Some(0xFF)
                && (4..=40).contains(&len)
                && let Some(name) = name
                && name.first() == Some(&b'C')
                && name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                let end = i + 6 + len;
                if end > to {
                    return;
                }
                if self.map.len() + 2 < MAX_OBJECTS {
                    let name = String::from_utf8_lossy(name).into_owned();
                    self.declare(name.clone());
                    let n = self.nodes.len();
                    self.nodes.push(Node::Skipped(name));
                    self.map.push(Entry::Object(n));
                    *at = end;
                    continue;
                }
            }
            *at = i + 1;
        }
    }

    /// A shape body here: matrix, `05`, the shape grammar, then the cubic hints.
    fn shape_body(&mut self) -> R<ShapeRecord> {
        self.shape_body_or_why().map_err(|_| Bad)
    }

    /// A shape body, or why it didn't decode.
    fn shape_body_or_why(&mut self) -> Result<ShapeRecord, shape::ShapeError> {
        let at05 = self.at.checked_add(24).ok_or(shape::ShapeError::Truncated)?;
        let record = shape::decode(self.d, at05)?;
        self.at = record.end;
        let n = self.u32().map_err(|_| shape::ShapeError::Truncated)? as usize;
        if n > shape::MAX_EDGES {
            return Err(shape::ShapeError::TooLarge);
        }
        self.skip(n.saturating_mul(32)).map_err(|_| shape::ShapeError::Truncated)?;
        Ok(record)
    }

    fn layer(&mut self) -> R<Node> {
        let base = self.base(KIND_LAYER)?;
        let name = self.string()?;
        let [current, hidden, locked] = self.arr::<3>()?;
        self.skip(4)?;
        let color = self.arr::<4>()?;
        let outline = self.u32()? != 0;
        self.skip(4)?;
        let kind = self.u8()?;
        let parent = self.obj(None)?;
        self.skip(2)?;
        Ok(Node::Layer(LayerRec { name, current: current != 0, hidden: hidden != 0, locked: locked != 0, outline, color, kind, parent, frames: base.children }))
    }

    fn frame(&mut self) -> R<Node> {
        let base = self.base(KIND_SHAPE)?;
        let start = self.at;
        let (raw, raw_error) = match self.shape_body_or_why() {
            Ok(r) => (Some(r), None),
            Err(why) => {
                // Artwork in a layout not decoded: find the frame's own fields.
                self.at = start;
                self.find_frame_tail()?;
                (None, Some(why))
            }
        };
        let mut f = self.frame_tail()?;
        f.raw = raw;
        f.raw_error = raw_error;
        f.elements = base.children;
        Ok(Node::Frame(Box::new(f)))
    }

    /// Moves to the frame fields after raw artwork that didn't decode: the first `18` from
    /// which the fields parse and the next object (or the layer's end) follows.
    fn find_frame_tail(&mut self) -> R<()> {
        let from = self.at;
        for i in from..self.d.len() {
            self.budget = self.budget.checked_sub(1).ok_or(Bad)?;
            if self.d.get(i) != Some(&0x18) || self.trials >= MAX_TRIALS {
                continue;
            }
            self.trials += 1;
            let snap = self.snap();
            self.at = i;
            let ok = self.frame_tail().is_ok() && self.continues_at(self.at, Some(KIND_LAYER));
            self.budget = self.budget.saturating_sub(self.at.saturating_sub(i));
            self.restore(snap);
            if ok {
                self.at = i;
                self.resyncs += 1;
                return Ok(());
            }
        }
        Err(Bad)
    }

    fn frame_tail(&mut self) -> R<FrameRec> {
        if self.u8()? != 0x18 {
            return Err(Bad);
        }
        let duration = self.u16()?;
        let tween = self.u8()?;
        let tween_flags = self.u8()?;
        let ease = self.i16()?;
        let sound = self.u16()?;
        let n = self.u16()?;
        let n = self.count(n as u32)?;
        let mut envelope = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            envelope.push((self.u32()?, self.u16()?, self.u16()?));
        }
        let loops = self.u16()?;
        let sync = self.u8()?;
        let in_point = self.u32()?;
        let out_point = self.u32()?;
        self.skip(2)?; // the sound's zoom in the editor
        let label = self.string()?;
        self.skip(16)?; // the frame actions' header
        let script = self.string()?;
        self.skip(8)?;
        let label_kind = self.u32()?;
        self.obj(None)?; // the shape tween's cache
        self.skip(4)?;
        let hints = match self.obj(None)? {
            Some(list) => self.hints_of(list),
            None => Vec::new(),
        };
        self.string()?;
        let mut flags = [0u32; 5];
        for v in flags.iter_mut() {
            *v = self.u32()?;
        }
        let mut eases = Vec::new();
        if flags[4] != 0 {
            for _ in 0..6 {
                let n = self.u32()?;
                let n = self.count(n)?;
                let mut curve = Vec::with_capacity(n.min(64));
                for _ in 0..n {
                    let mut a = [0f64; 6];
                    for v in a.iter_mut() {
                        *v = self.f64()?;
                    }
                    curve.push(a);
                }
                eases.push(curve);
            }
        }
        Ok(FrameRec { duration, tween, tween_flags, ease, sound, envelope, loops, sync, in_point, out_point, label, label_kind, eases, raw: None, raw_error: None, hints, script, elements: Vec::new() })
    }

    /// The hints in a decoded hint list, in order.
    fn hints_of(&self, list: usize) -> Vec<HintRec> {
        let Some(Node::List(items)) = self.nodes.get(list) else { return Vec::new() };
        items.iter().filter_map(|&i| match self.nodes.get(i) {
            Some(Node::Hint(a, b)) => Some((*a, *b)),
            _ => None,
        }).collect()
    }

    /// Whether the bytes at `i` are where an object's parent (of kind `parent`) continues: its
    /// end or a next sibling's head, maybe behind references to objects read before (siblings
    /// too). Without a known parent, anything goes.
    fn continues_at(&self, i: usize, parent: Option<u8>) -> bool {
        let Some(k) = parent else { return true };
        let mut i = i;
        for _ in 0..=MAX_REF_RUN {
            if self.parent_end_at(i, k) || self.sibling_head_at(i) {
                return true;
            }
            match self.reference_at(i).and_then(|len| i.checked_add(len)) {
                Some(next) => i = next,
                None => return false,
            }
        }
        false
    }

    /// A text record (its head here), decoded by [`crate::text`]; one in a layout not seen,
    /// or that doesn't end where its parent continues, is skipped.
    fn text(&mut self, parent: Option<u8>) -> R<Node> {
        if let Ok(t) = crate::text::decode(self.d, self.at)
            && self.continues_at(t.end, parent)
        {
            self.budget = self.budget.saturating_sub(t.end.saturating_sub(self.at));
            self.at = t.end;
            return Ok(Node::Text(Box::new(t)));
        }
        self.resync(parent)?;
        Ok(Node::Skipped("CPicText".into()))
    }

    /// A bitmap instance: the common head (kind 02, no children), its matrix, its media item
    /// id and a byte (0 on every corpus instance); one in a layout not seen is skipped.
    fn bitmap(&mut self, parent: Option<u8>) -> R<Node> {
        let snap = self.snap();
        let decoded = (|| -> R<BitmapRec> {
            let base = self.base(KIND_BITMAP)?;
            if !base.children.is_empty() {
                return Err(Bad);
            }
            let matrix = self.matrix()?;
            let item = self.u16()?;
            self.skip(1)?; // 0 throughout the corpus
            Ok(BitmapRec { matrix, item })
        })();
        match decoded {
            Ok(b) if self.continues_at(self.at, parent) => Ok(Node::Bitmap(b)),
            _ => {
                self.restore(snap);
                self.resync(parent)?;
                Ok(Node::Skipped("CPicBitmap".into()))
            }
        }
    }

    fn instance(&mut self, class: InstanceClass, parent: Option<u8>) -> R<Node> {
        let base = self.base(KIND_INSTANCE)?;
        let matrix = self.matrix()?;
        let first_frame = self.u16()?;
        let loop_mode = self.u16()?;
        self.skip(1)?;
        let mut color = ColorRec::default();
        for i in 0..4 {
            let (m, a) = (self.i16()?, self.i16()?);
            if let (Some(mm), Some(aa)) = (color.mul.get_mut(i), color.add.get_mut(i)) {
                *mm = m;
                *aa = a;
            }
        }
        color.style = self.u8()?;
        self.skip(1)?;
        color.percent = self.i16()?;
        color.tint = self.arr()?;
        let name = self.string()?;
        let item = self.u16()?;
        self.skip(3)?;
        if self.u8()? != 0 {
            if self.u32()? != 1 {
                return Err(Bad);
            }
            let n = self.u32()?;
            for _ in 0..self.count(n)? {
                if self.u8()? != 0x05 {
                    return Err(Bad);
                }
                self.string()?;
                self.string()?;
            }
        }
        self.skip(1)?;
        let filters = self.u8()?;
        let mut rec = InstanceRec {
            class,
            locked: base.flag & 0x04 != 0,
            matrix,
            point: (base.x, base.y),
            first_frame,
            loop_mode,
            color,
            name,
            item,
            blend: 1,
            filters,
            script: String::new(),
            track_as_menu: false,
            extras_skipped: false,
        };
        if filters != 0 {
            // A filter list (layout not decoded), the blend after it, and any settings.
            self.resync(parent)?;
            rec.extras_skipped = true;
            return Ok(Node::Instance(Box::new(rec)));
        }
        rec.blend = self.u8()?;
        self.skip(2)?;
        if class != InstanceClass::Graphic {
            let snap = self.snap();
            if !self.extras(&mut rec, parent) {
                // Settings in a layout not seen: skipped whole.
                self.restore(snap);
                rec.script.clear();
                rec.track_as_menu = false;
                self.resync(parent)?;
                rec.extras_skipped = true;
            }
        }
        Ok(Node::Instance(Box::new(rec)))
    }

    /// A button's or movie clip's own settings after the common instance fields: whether they
    /// decoded (a movie clip's component settings after its name are skipped, and say so).
    fn extras(&mut self, rec: &mut InstanceRec, parent: Option<u8>) -> bool {
        let want = if rec.class == InstanceClass::Button { BUTTON_EXTRAS } else { SPRITE_EXTRAS };
        let head = (|| -> R<()> {
            if self.u8()? != want {
                return Err(Bad);
            }
            self.skip(16)?; // the actions' header
            rec.script = self.string()?;
            if rec.class == InstanceClass::Button {
                rec.track_as_menu = self.u8()? != 0;
            }
            let name = self.string()?;
            if !name.trim().is_empty() {
                rec.name = name;
            }
            Ok(())
        })();
        if head.is_err() {
            return false;
        }
        if rec.class == InstanceClass::Button {
            return self.u32().is_ok_and(|z| z == 0) && self.continues_at(self.at, parent);
        }
        // A movie clip's component settings: not decoded.
        if self.resync(parent).is_err() {
            return false;
        }
        rec.extras_skipped = true;
        true
    }

    /// A shape tween's cache: version 2, two matrices, 8 bytes, its segments (objects), then
    /// its fill styles (RGBA, kind, a byte; a gradient adds a matrix, a count and 5-byte
    /// stops) and strokes (10 bytes each).
    fn morph(&mut self) -> R<Node> {
        if self.u8()? != 0x02 {
            return Err(Bad);
        }
        self.skip(48 + 8)?;
        let n = self.u16()?;
        for _ in 0..n {
            self.obj(None)?;
        }
        self.skip(2)?;
        let fills = self.u16()?;
        for _ in 0..fills {
            self.skip(4)?;
            let kind = self.u8()?;
            self.skip(1)?;
            match kind {
                0x00 => {}
                0x10 | 0x12 => {
                    self.skip(24)?;
                    let stops = self.u8()? as usize;
                    self.skip(stops * 5)?;
                }
                _ => return Err(Bad),
            }
        }
        let strokes = self.u16()? as usize;
        self.skip(strokes * 10)?;
        Ok(Node::Morph)
    }
}
