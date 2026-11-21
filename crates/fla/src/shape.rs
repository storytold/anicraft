//! `CPicShape` records: a shape's matrix, fill and stroke styles and its edge list, with
//! straight and quadratic edges (ADR 0096).
//!
//! Learned differentially from the corpus's legacy files and their Animate twins (every
//! decoded edge, style index and matrix equals the twin's XFL `DOMShape`): no Flash or
//! Animate code involved. A shape body sits behind its MFC-style object header (the class
//! name `CPicShape` or a `0x80nn` class reference, then 15 bytes: `04`, a flag byte, twelve
//! bytes, `03`), and reads:
//!
//! ```text
//! matrix      a b c d : i32 16.16 · tx ty : i32 twips
//! 05
//! edge count  u32 (the edges that draw; a decode must find exactly this many)
//! fills       u16 n, n × fill
//! strokes     u16 n, n × { RGBA, u16 width (twips), 10 style bytes, fill }
//! edges       records up to a 00 byte
//! ```
//!
//! A stroke's ten style bytes:
//!
//! ```text
//! [0..2]  u16 dash   a dashed line's dash length, in twips (0: not dashed)
//! [2..4]  u16        dashed: the gap length in twips · otherwise (pattern << 4) | type
//! [4]     00         (zero in every corpus stroke)
//! [5]     00 or 03   not understood (no effect on the drawing found)
//! [6]     cap        00 none, 01 round, 02 square
//! [7]     join       00 miter, 01 round, 02 bevel
//! [8]     00         (zero in every corpus stroke)
//! [9]     miter limit (00: the default 3)
//! ```
//!
//! A dash length marks a dashed line (XFL's `DashedStroke` `dash1`, `dash2`; Animate's
//! default is 6 and 6 px). Otherwise the low four bits of the second field are the stroke
//! type, in the order of Animate's stroke style list: 0 solid, 2 dotted, 5 hatched (both
//! seen in the corpus), with 3 and 4 presumably ragged and stipple. A dotted line's pattern
//! is its dot spacing in tenths of a pixel (`0x1E2`: type 2, 30 → `dotSpace="3"`); a
//! hatched line's packs its hatch settings (not decoded). Ragged, stipple and hatched lines
//! draw solid, as the XFL importer draws them; a type or byte 4 or 8 not seen in the corpus
//! is a style not decoded yet, drawn solid. Both are counted. Hairlines are a width of one
//! twip.
//!
//! A fill is RGBA, a kind byte (00 solid, 10 linear, 12 radial) and a byte not yet
//! understood; a gradient adds its matrix (as above), a stop count (u8), the focal point
//! (i32, in 1/255 of the radius), four bytes (zero in every corpus file) and the stops
//! (u8 ratio in 1/255, RGBA).
//!
//! Each edge record starts with a flag byte of four 2-bit fields, low to high: the pen
//! move, the control point, the anchor, the style change. A point field says how its
//! point is stored, relative to the pen: `00` absent, `01` i16 pair in 1/256 twip, `10`
//! i32 pair in 1/256 twip, `11` i16 pair in 1/2 twip. The style field `11` carries three
//! u8 style indices (stroke, left fill, right fill — XFL's `strokeStyle`, `fillStyle0`,
//! `fillStyle1`), `01` three u16 indices (bit 15 is a flag that doesn't change the index);
//! the style data comes first, then the move, control and anchor points. A record with an
//! anchor draws a line, or a quadratic curve when it has a control point, and the pen moves
//! to the anchor. The pen starts at the origin.

use anicraft_doc::stroke_style::{DEFAULT_MITER_LIMIT, MAX_MITER_LIMIT, MIN_MITER_LIMIT};
use anicraft_doc::{Cap, Dash, GradientStop, Join, Paint, Rgba, Shape, Stroke, StrokeStyle, MAX_FOCAL};
use anicraft_geom::{Affine, FillRule, Path, Vec2};
use std::collections::HashMap;
use std::sync::Arc;

/// Edge coordinates count 1/256 twip: this many make a pixel.
const UNITS_PER_PX: f64 = 256.0 * 20.0;
/// Half the side of the square a gradient is defined on, in pixels (16384 twips / 20).
const GRADIENT_HALF: f32 = 819.2;
/// The object header between a shape's class tag and its matrix.
const HEADER: usize = 15;
const MATRIX: usize = 24;
/// The most fill or stroke styles one shape may list.
pub const MAX_STYLES: usize = 4096;
/// The most edges one shape may list.
pub const MAX_EDGES: usize = 1 << 20;

/// One segment, in pixels: a line (`ctrl` is `None`) or a quadratic curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub from: Vec2,
    pub ctrl: Option<Vec2>,
    pub to: Vec2,
}

impl Seg {
    fn reversed(self) -> Seg {
        Seg { from: self.to, ctrl: self.ctrl, to: self.from }
    }
}

/// One drawn edge and the styles on it (1-based indices; 0 is none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub stroke: u16,
    pub fill_left: u16,
    pub fill_right: u16,
    pub seg: Seg,
}

/// A decoded shape record.
#[derive(Clone, Debug)]
pub struct ShapeRecord {
    pub matrix: Affine,
    /// Fill styles in index order (`None`: a gradient without stops, which draws nothing).
    pub fills: Vec<Option<Paint>>,
    pub strokes: Vec<Stroke>,
    /// Each stroke's kind as stored, in the same order (the drawn [`Stroke`] holds what
    /// AniCraft draws of it).
    pub stroke_kinds: Vec<StrokeKind>,
    pub edges: Vec<Edge>,
    /// What was drawn the nearest way AniCraft can, as report kinds.
    pub approximations: Vec<&'static str>,
    /// Where the record's edge list ended (the byte after its `00` terminator).
    pub end: usize,
}

/// Why a shape record didn't decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ShapeError {
    #[error("not a shape record")]
    NotShape,
    #[error("the record ends early")]
    Truncated,
    #[error("a fill of kind {0:#04x}")]
    FillKind(u8),
    #[error("an edge record with an unknown style-index width")]
    StyleWidth,
    #[error("an edge record with a control point but no anchor")]
    Dangling,
    #[error("the edges found don't match the record's edge count")]
    Count,
    #[error("more styles or edges than a shape may hold")]
    TooLarge,
}

/// Bounded little-endian reads: every read past the end is [`ShapeError::Truncated`].
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], ShapeError> {
        let end = self.at.checked_add(N).ok_or(ShapeError::Truncated)?;
        let s = self.data.get(self.at..end).ok_or(ShapeError::Truncated)?;
        self.at = end;
        <[u8; N]>::try_from(s).map_err(|_| ShapeError::Truncated)
    }
    fn u8(&mut self) -> Result<u8, ShapeError> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, ShapeError> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }
    fn i16(&mut self) -> Result<i16, ShapeError> {
        Ok(i16::from_le_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32, ShapeError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn i32(&mut self) -> Result<i32, ShapeError> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }
    /// A matrix: a, b, c, d in 16.16 fixed point, then tx, ty in twips.
    fn matrix(&mut self) -> Result<Affine, ShapeError> {
        let mut m = [0f32; 6];
        for (i, v) in m.iter_mut().enumerate() {
            let raw = self.i32()? as f32;
            *v = if i < 4 { raw / 65536.0 } else { raw / 20.0 };
        }
        Ok(Affine { m })
    }
    /// A point stored as `code` says (relative, in 1/256 twip), or `None` for `00`.
    fn point(&mut self, code: u8) -> Result<Option<(i64, i64)>, ShapeError> {
        Ok(match code & 3 {
            1 => Some((self.i16()? as i64, self.i16()? as i64)),
            2 => Some((self.i32()? as i64, self.i32()? as i64)),
            3 => Some((self.i16()? as i64 * 128, self.i16()? as i64 * 128)),
            _ => None,
        })
    }
}

fn rgba(b: [u8; 4]) -> Rgba {
    Rgba::new(b[0] as f32 / 255.0, b[1] as f32 / 255.0, b[2] as f32 / 255.0, b[3] as f32 / 255.0)
}

fn px(p: (i64, i64)) -> Vec2 {
    Vec2::new((p.0 as f64 / UNITS_PER_PX) as f32, (p.1 as f64 / UNITS_PER_PX) as f32)
}

fn add(a: (i64, i64), b: (i64, i64)) -> (i64, i64) {
    (a.0.saturating_add(b.0), a.1.saturating_add(b.1))
}

/// A fill style: RGBA, kind, a byte not yet understood, and a gradient's definition.
fn fill(r: &mut Reader, approximations: &mut Vec<&'static str>) -> Result<Option<Paint>, ShapeError> {
    let color = rgba(r.bytes()?);
    let kind = r.u8()?;
    r.u8()?; // varies on solid fills; no observed effect
    match kind {
        0x00 => Ok(Some(Paint::Solid(color))),
        0x10 | 0x12 => {
            let m = r.matrix()?;
            let count = r.u8()?;
            let focal = r.i32()? as f32 / 255.0;
            if r.bytes::<4>()? != [0; 4] {
                approximations.push("gradient overflow and colour-space settings (drawn with the defaults)");
            }
            let mut stops = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let offset = r.u8()? as f32 / 255.0;
                stops.push(GradientStop { offset, color: rgba(r.bytes()?) });
            }
            if stops.is_empty() {
                return Ok(None);
            }
            let mut paint = if kind == 0x10 { Paint::linear(Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0), stops) } else { Paint::radial(Vec2::ZERO, 1.0, stops) };
            if let Paint::Radial { focal: f, .. } = &mut paint {
                *f = if focal.is_finite() { focal.clamp(-MAX_FOCAL, MAX_FOCAL) } else { 0.0 };
            }
            Ok(Some(paint.with_frame(&m.mul(&Affine::scale(GRADIENT_HALF, GRADIENT_HALF)), None)))
        }
        other => Err(ShapeError::FillKind(other)),
    }
}

/// A stroke's kind and pattern, from its style bytes (the module docs give the layout).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrokeKind {
    Solid,
    /// Dash and gap lengths, in pixels.
    Dashed { dash: f32, gap: f32 },
    /// Dot spacing, in pixels.
    Dotted { spacing: f32 },
    /// Types 3, 4 and 5 (drawn solid): their patterns aren't decoded.
    Ragged,
    Stipple,
    Hatched,
    /// A type or style byte not seen in the corpus (drawn solid).
    Unknown,
}

impl StrokeKind {
    pub fn of(style: &[u8; 10]) -> StrokeKind {
        if style[4] != 0 || style[8] != 0 {
            return StrokeKind::Unknown;
        }
        let dash = u16::from_le_bytes([style[0], style[1]]);
        let field = u16::from_le_bytes([style[2], style[3]]);
        if dash != 0 {
            return StrokeKind::Dashed { dash: dash as f32 / 20.0, gap: field as f32 / 20.0 };
        }
        match (field & 0xF, field >> 4) {
            (0, 0) => StrokeKind::Solid,
            (2, spacing) => StrokeKind::Dotted { spacing: spacing as f32 / 10.0 },
            (3, _) => StrokeKind::Ragged,
            (4, _) => StrokeKind::Stipple,
            (5, _) => StrokeKind::Hatched,
            _ => StrokeKind::Unknown,
        }
    }

    /// The pattern AniCraft draws (`None`: a solid line), and what the drawing leaves out.
    pub fn dash(self) -> (Option<Dash>, Option<&'static str>) {
        match self {
            StrokeKind::Solid => (None, None),
            StrokeKind::Dashed { dash, gap } => (Dash::dashed(dash, gap), None),
            StrokeKind::Dotted { spacing } => (Dash::dotted(spacing), None),
            StrokeKind::Ragged | StrokeKind::Stipple | StrokeKind::Hatched => (None, Some("ragged, stipple and hatched strokes (drawn solid)")),
            StrokeKind::Unknown => (None, Some("stroke styles not decoded yet (drawn solid)")),
        }
    }
}

/// A stroke style: RGBA, width in twips, ten bytes of style (dash and gap, cap, join, miter
/// limit), then the stroke's own fill (for a solid stroke, its colour again).
fn stroke(r: &mut Reader, approximations: &mut Vec<&'static str>) -> Result<(Stroke, StrokeKind), ShapeError> {
    let mut color = rgba(r.bytes()?);
    let width = (r.u16()? as f32 / 20.0).clamp(0.05, 1000.0);
    let style: [u8; 10] = r.bytes()?;
    let cap = match style[6] {
        0 => Cap::Butt,
        2 => Cap::Square,
        _ => Cap::Round,
    };
    let join = match style[7] {
        0 => Join::Miter,
        2 => Join::Bevel,
        _ => Join::Round,
    };
    let miter_limit = match style[9] {
        0 => DEFAULT_MITER_LIMIT,
        m => (m as f32).clamp(MIN_MITER_LIMIT, MAX_MITER_LIMIT),
    };
    let kind = StrokeKind::of(&style);
    let (dash, left_out) = kind.dash();
    approximations.extend(left_out);
    match fill(r, approximations)? {
        Some(Paint::Solid(_)) | None => {}
        Some(gradient) => {
            approximations.push("gradient strokes (drawn in one colour)");
            color = gradient.primary_color();
        }
    }
    Ok((Stroke::new(color, width).with_style(StrokeStyle { cap, join, miter_limit, dash }), kind))
}

/// Decodes the shape body whose matrix ends at `at` (the offset of its `05` byte).
pub fn decode(data: &[u8], at: usize) -> Result<ShapeRecord, ShapeError> {
    let mut r = Reader { data, at: at.checked_sub(MATRIX).ok_or(ShapeError::NotShape)? };
    let matrix = r.matrix()?;
    if r.u8()? != 0x05 {
        return Err(ShapeError::NotShape);
    }
    let count = r.u32()? as usize;
    if count > MAX_EDGES {
        return Err(ShapeError::TooLarge);
    }
    let mut approximations = Vec::new();
    let fill_count = r.u16()? as usize;
    if fill_count > MAX_STYLES {
        return Err(ShapeError::TooLarge);
    }
    let mut fills = Vec::with_capacity(fill_count.min(64));
    for _ in 0..fill_count {
        fills.push(fill(&mut r, &mut approximations)?);
    }
    let stroke_count = r.u16()? as usize;
    if stroke_count > MAX_STYLES {
        return Err(ShapeError::TooLarge);
    }
    let mut strokes = Vec::with_capacity(stroke_count.min(64));
    let mut stroke_kinds = Vec::with_capacity(stroke_count.min(64));
    for _ in 0..stroke_count {
        let (stroke, kind) = stroke(&mut r, &mut approximations)?;
        strokes.push(stroke);
        stroke_kinds.push(kind);
    }
    let mut edges = Vec::with_capacity(count.min(4096));
    let (mut pen, mut styles) = ((0i64, 0i64), (0u16, 0u16, 0u16));
    loop {
        let flag = r.u8()?;
        if flag == 0 {
            break;
        }
        match flag >> 6 {
            0 => {}
            1 => styles = (r.u16()? & 0x7FFF, r.u16()? & 0x7FFF, r.u16()? & 0x7FFF),
            3 => styles = (r.u8()? as u16, r.u8()? as u16, r.u8()? as u16),
            _ => return Err(ShapeError::StyleWidth),
        }
        if let Some(d) = r.point(flag)? {
            pen = add(pen, d);
        }
        let ctrl = r.point(flag >> 2)?;
        let Some(anchor) = r.point(flag >> 4)? else {
            if ctrl.is_some() {
                return Err(ShapeError::Dangling);
            }
            continue; // a move or a style change alone
        };
        if edges.len() >= count {
            return Err(ShapeError::Count);
        }
        let to = add(pen, anchor);
        let seg = Seg { from: px(pen), ctrl: ctrl.map(|c| px(add(pen, c))), to: px(to) };
        edges.push(Edge { stroke: styles.0, fill_left: styles.1, fill_right: styles.2, seg });
        pen = to;
    }
    if edges.len() != count {
        return Err(ShapeError::Count);
    }
    Ok(ShapeRecord { matrix, fills, strokes, stroke_kinds, edges, approximations, end: r.at })
}

/// The shape body behind an object whose class tag or name ends at `at`: the 15-byte
/// header (`04` … `03`), the matrix, then the `05` byte. Returns the `05`'s offset.
pub fn body_after_tag(data: &[u8], at: usize) -> Option<usize> {
    let header = data.get(at..at.checked_add(HEADER)?)?;
    if header.first() != Some(&0x04) || header.last() != Some(&0x03) {
        return None;
    }
    let body = at + HEADER + MATRIX;
    (data.get(body) == Some(&0x05)).then_some(body)
}

/// Every shape body in a stream that decodes whole — `CPicShape` objects and the raw
/// artwork frames carry — found by their header and checked by a full decode (the edge
/// count, then the terminator). For inspection and differential tests: the record-level
/// pass that places them on frames is later work.
pub fn find_all(data: &[u8]) -> Vec<ShapeRecord> {
    let mut out = Vec::new();
    let mut at = HEADER + MATRIX;
    while at < data.len() {
        if data.get(at) == Some(&0x05)
            && let Some(start) = at.checked_sub(HEADER + MATRIX)
            && body_after_tag(data, start) == Some(at)
            && let Ok(record) = decode(data, at)
        {
            at = record.end.max(at + 1);
            out.push(record);
            continue;
        }
        at += 1;
    }
    out
}

/// Hash key for an end point (to 1/100 px).
fn key(p: Vec2) -> (i64, i64) {
    ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64)
}

fn push(path: &mut Path, s: &Seg) {
    match s.ctrl {
        Some(c) => path.quad_to(c, s.to),
        None => path.line_to(s.to),
    };
}

/// Fill segments chained into closed loops, end to start: each step takes the first unused
/// segment that starts where the last ended, found through a per-vertex cursor (segments only
/// ever become used, so the cursor never has to look back).
fn fill_loops(segs: &[Seg]) -> Path {
    let mut by_start: HashMap<(i64, i64), (usize, Vec<usize>)> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        // Zero-length segments carry no area and would confuse the chaining.
        if key(s.from) != key(s.to) {
            by_start.entry(key(s.from)).or_default().1.push(i);
        }
    }
    let mut used = vec![false; segs.len()];
    let mut path = Path::new();
    for (first, start) in segs.iter().enumerate() {
        if used.get(first).copied().unwrap_or(true) || key(start.from) == key(start.to) {
            continue;
        }
        path.move_to(start.from);
        let mut at = first;
        while let Some(s) = segs.get(at) {
            if let Some(u) = used.get_mut(at) {
                *u = true;
            }
            push(&mut path, s);
            if key(s.to) == key(start.from) {
                break;
            }
            let next = by_start.get_mut(&key(s.to)).and_then(|(cursor, list)| {
                while list.get(*cursor).is_some_and(|&j| used.get(j).copied().unwrap_or(true)) {
                    *cursor += 1;
                }
                list.get(*cursor).copied()
            });
            match next {
                Some(j) => at = j,
                None => break,
            }
        }
        path.close();
    }
    path
}

/// Stroke segments in drawing order, a new subpath wherever one doesn't start at the last's end.
fn stroke_run<'a>(segs: impl Iterator<Item = &'a Seg>) -> Path {
    let mut path = Path::new();
    let mut last: Option<Vec2> = None;
    for s in segs {
        if last.is_none_or(|l| key(l) != key(s.from)) {
            path.move_to(s.from);
        }
        push(&mut path, s);
        last = Some(s.to);
    }
    path
}

impl ShapeRecord {
    /// The region of fill style `style` as closed loops: the edges with the fill on their
    /// right as drawn, those with it on their left reversed, chained end to start — the
    /// same rebuild as the XFL importer's (`anicraft-xfl`'s `edges::fill_path`).
    pub fn fill_path(&self, style: u16) -> Path {
        let mut segs: Vec<Seg> = Vec::new();
        for e in &self.edges {
            if e.fill_right == style {
                segs.push(e.seg);
            }
            if e.fill_left == style {
                segs.push(e.seg.reversed());
            }
        }
        fill_loops(&segs)
    }

    /// The edges of stroke style `style`, in drawing order.
    pub fn stroke_path(&self, style: u16) -> Path {
        stroke_run(self.edges.iter().filter(|e| e.stroke == style).map(|e| &e.seg))
    }

    /// The record as AniCraft shapes: one per fill style (bottom), then one per stroke
    /// style, as Animate draws them. The edges are sorted by style once, in edge order (so
    /// each style's segments are exactly those [`ShapeRecord::fill_path`] and
    /// [`ShapeRecord::stroke_path`] gather), not scanned again for every style.
    pub fn shapes(&self) -> Vec<Shape> {
        let mut fill_segs: Vec<Vec<Seg>> = vec![Vec::new(); self.fills.len()];
        let mut stroke_segs: Vec<Vec<&Seg>> = vec![Vec::new(); self.strokes.len()];
        // Style n (1-based) is slot n - 1; 0 is none, and a style the record doesn't list is
        // drawn by nothing.
        let slot = |style: u16| usize::from(style).checked_sub(1);
        for e in &self.edges {
            if let Some(v) = slot(e.fill_right).and_then(|i| fill_segs.get_mut(i)) {
                v.push(e.seg);
            }
            if let Some(v) = slot(e.fill_left).and_then(|i| fill_segs.get_mut(i)) {
                v.push(e.seg.reversed());
            }
            if let Some(v) = slot(e.stroke).and_then(|i| stroke_segs.get_mut(i)) {
                v.push(&e.seg);
            }
        }
        let mut out = Vec::new();
        for (paint, segs) in self.fills.iter().zip(&fill_segs) {
            let Some(paint) = paint else { continue };
            let path = fill_loops(segs);
            if !path.is_empty() {
                out.push(Shape { path: Arc::new(path), fill: Some(paint.clone()), stroke: None, fill_rule: FillRule::EvenOdd, merge: false, warp: None });
            }
        }
        for (stroke, segs) in self.strokes.iter().zip(&stroke_segs) {
            let path = stroke_run(segs.iter().copied());
            if !path.is_empty() {
                out.push(Shape { path: Arc::new(path), fill: None, stroke: Some(stroke.clone()), fill_rule: FillRule::NonZero, merge: false, warp: None });
            }
        }
        out
    }
}
