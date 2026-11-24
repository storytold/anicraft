//! Native multi-layer Adobe Photoshop `.psd` v1 importer (`P-038`).
//!
//! Written in pure safe Rust from the public Adobe Photoshop File Formats Specification (v1 `8BPS`
//! container): reads 8-bit RGB and Grayscale documents up to [`MAX_BITMAP_EDGE`] per side,
//! bounded by [`MAX_PSD_BYTES`] and [`MAX_PSD_LAYERS`], supporting Raw (`0`) and PackBits RLE (`1`)
//! scanlines, layer bounds, alpha channels (`-1`), opacity, visibility, blend modes, Pascal +
//! UTF-16BE `'luni'` layer names, `'lsct'`/`'lsdk'` layer groups, and merged-composite fallback.

use crate::params::*;
use crate::{CommandSpec, EngineError, Result, Session};
use anicraft_doc::{
    Bitmap, BitmapId, BlendMode, ColorEffect, Document, Element, ElementId, ElementKind, Keyframe,
    Layer, LayerId, LayerKind, LoopMode, MAX_BITMAP_EDGE, MAX_LAYER_DEPTH, Paint, Rgba, Shape,
    Symbol, SymbolId, SymbolKind, Timeline,
};
use anicraft_geom::{Affine, FillRule, Path as GeomPath};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Largest `.psd` file accepted (256 MiB).
pub const MAX_PSD_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum layer records parsed in one `.psd` file.
pub const MAX_PSD_LAYERS: usize = 512;
/// Maximum channels per layer or merged image.
const MAX_CHANNELS: usize = 16;
/// Maximum UTF-16 code units read from a `'luni'` layer name.
const MAX_NAME_CODE_UNITS: usize = 512;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "file.import_psd",
        label: "Import Photoshop (.psd)…",
        menu: "File/Import",
        shortcut: None,
        params: "{path, mode?: \"layers\" (default) | \"flattened\", x?, y?, cx?, cy?}",
        mutates: true,
        run: import_psd_cmd,
    }]
}

/// Whether `path` has a `.psd` extension.
pub fn is_psd(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("psd"))
}

/// Photoshop section-divider kind (`'lsct'` / `'lsdk'`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsdSectionKind {
    /// Ordinary pixel layer (`0` or absent).
    Normal,
    /// Folder group header (`1` = open, `2` = closed; PSD stores it at the top of the group).
    Folder { open: bool },
    /// Invisible bounding marker at the bottom of a folder group (`3`, `</Layer group>`).
    FolderEnd,
}

/// One parsed PSD layer record and its decoded straight-RGBA8 pixels (when non-empty).
#[derive(Clone, Debug, PartialEq)]
pub struct PsdLayer {
    pub name: String,
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    pub width: u32,
    pub height: u32,
    pub opacity: u8,
    pub visible: bool,
    pub clipped: bool,
    pub blend: BlendMode,
    pub section: PsdSectionKind,
    /// `width * height * 4` straight-alpha RGBA8 bytes when `width > 0 && height > 0` on a normal layer.
    pub rgba: Vec<u8>,
}

/// A parsed `.psd` v1 document.
#[derive(Clone, Debug)]
pub struct PsdDocument {
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub color_mode: u16,
    /// Layers in PSD file order (bottom-to-top: index `0` is the bottom-most layer).
    pub layers: Vec<PsdLayer>,
    /// Optional merged composite RGBA8 (`width * height * 4`), decoded when present.
    pub merged_rgba: Option<Vec<u8>>,
}

// ----- Safe bounded byte cursor -----------------------------------------------------------------

#[derive(Clone, Copy)]
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> &'a [u8] {
        self.buf.get(self.pos..).unwrap_or(&[])
    }

    fn take(&mut self, n: usize) -> std::result::Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or_else(|| "PSD offset overflow".to_string())?;
        let slice = self.buf.get(self.pos..end).ok_or_else(|| "truncated PSD data".to_string())?;
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> std::result::Result<u8, String> {
        let b = self.take(1)?;
        b.first().copied().ok_or_else(|| "truncated PSD byte".to_string())
    }

    fn read_u16_be(&mut self) -> std::result::Result<u16, String> {
        let b = self.take(2)?;
        let arr: [u8; 2] = [b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)];
        Ok(u16::from_be_bytes(arr))
    }

    fn read_i16_be(&mut self) -> std::result::Result<i16, String> {
        Ok(self.read_u16_be()? as i16)
    }

    fn read_u32_be(&mut self) -> std::result::Result<u32, String> {
        let b = self.take(4)?;
        let arr: [u8; 4] = [
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
            b.get(2).copied().unwrap_or(0),
            b.get(3).copied().unwrap_or(0),
        ];
        Ok(u32::from_be_bytes(arr))
    }

    fn read_i32_be(&mut self) -> std::result::Result<i32, String> {
        Ok(self.read_u32_be()? as i32)
    }

    fn read_fourcc(&mut self) -> std::result::Result<[u8; 4], String> {
        let b = self.take(4)?;
        Ok([
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
            b.get(2).copied().unwrap_or(0),
            b.get(3).copied().unwrap_or(0),
        ])
    }
}

/// Maps a 4-byte Photoshop blend mode key to AniCraft's [`BlendMode`].
pub fn map_blend_mode(fourcc: &[u8; 4]) -> BlendMode {
    match fourcc {
        b"norm" | b"pass" => BlendMode::Normal,
        b"mul " => BlendMode::Multiply,
        b"scrn" => BlendMode::Screen,
        b"over" | b"sLit" => BlendMode::Overlay,
        b"dark" => BlendMode::Darken,
        b"lite" => BlendMode::Lighten,
        b"lddg" | b"add " => BlendMode::Add,
        b"fsub" => BlendMode::Subtract,
        b"diff" | b"smud" => BlendMode::Difference,
        b"hLit" | b"hard" => BlendMode::HardLight,
        _ => BlendMode::Normal,
    }
}

/// Decodes one PackBits RLE scanline into `out` (`expected_len` bytes).
fn decode_packbits_row(src: &[u8], out: &mut [u8]) -> std::result::Result<(), String> {
    let mut si = 0usize;
    let mut di = 0usize;
    while di < out.len() {
        let Some(&hdr_u8) = src.get(si) else {
            return Err("truncated PackBits RLE scanline".into());
        };
        si += 1;
        let n = hdr_u8 as i8;
        if n >= 0 {
            let count = (n as usize) + 1;
            let end_s = si.checked_add(count).ok_or_else(|| "RLE literal overflow".to_string())?;
            let end_d = di.checked_add(count).ok_or_else(|| "RLE output overflow".to_string())?;
            let lit = src.get(si..end_s).ok_or_else(|| "truncated PackBits literal run".to_string())?;
            let dst = out.get_mut(di..end_d).ok_or_else(|| "PackBits literal run exceeds row width".to_string())?;
            dst.copy_from_slice(lit);
            si = end_s;
            di = end_d;
        } else if n > -128 {
            let count = ((-i32::from(n)) as usize) + 1;
            let val = *src.get(si).ok_or_else(|| "truncated PackBits repeat byte".to_string())?;
            si += 1;
            let end_d = di.checked_add(count).ok_or_else(|| "RLE repeat overflow".to_string())?;
            let dst = out.get_mut(di..end_d).ok_or_else(|| "PackBits repeat run exceeds row width".to_string())?;
            dst.fill(val);
            di = end_d;
        }
        // n == -128 is a PackBits no-op.
    }
    Ok(())
}

/// Decodes one channel plane of `w × h` 8-bit samples (starts with `u16` compression).
fn decode_channel_plane(data: &[u8], w: u32, h: u32) -> std::result::Result<Vec<u8>, String> {
    let total = (w as usize)
        .checked_mul(h as usize)
        .ok_or_else(|| "PSD channel dimensions overflow".to_string())?;
    if total == 0 {
        return Ok(Vec::new());
    }
    if w > MAX_BITMAP_EDGE || h > MAX_BITMAP_EDGE {
        return Err(format!("PSD layer {w}×{h} exceeds {MAX_BITMAP_EDGE} px limit"));
    }
    let mut c = Cursor::new(data);
    let comp = c.read_u16_be()?;
    match comp {
        0 => {
            let raw = c.take(total)?;
            Ok(raw.to_vec())
        }
        1 => {
            let mut row_lens = Vec::with_capacity(h as usize);
            for _ in 0..h {
                row_lens.push(c.read_u16_be()? as usize);
            }
            let mut out = vec![0u8; total];
            let stride = w as usize;
            for (row_idx, &rlen) in row_lens.iter().enumerate() {
                let row_src = c.take(rlen)?;
                let start = row_idx * stride;
                let dst = out
                    .get_mut(start..start + stride)
                    .ok_or_else(|| "PSD row slice out of bounds".to_string())?;
                decode_packbits_row(row_src, dst)?;
            }
            Ok(out)
        }
        other => Err(format!(
            "unsupported PSD channel compression {other} (only Raw=0 and RLE=1 are supported)"
        )),
    }
}

/// Decodes the merged composite section at the end of a `.psd` file into straight RGBA8.
fn decode_merged_section(
    c: &mut Cursor<'_>,
    w: u32,
    h: u32,
    channels: u16,
    color_mode: u16,
) -> std::result::Result<Vec<u8>, String> {
    if c.remaining().len() < 2 {
        return Err("missing PSD merged image data".into());
    }
    let ch_count = (channels as usize).clamp(1, MAX_CHANNELS);
    let pixels = (w as usize)
        .checked_mul(h as usize)
        .ok_or_else(|| "PSD merged dimensions overflow".to_string())?;
    let comp = c.read_u16_be()?;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(ch_count);
    match comp {
        0 => {
            for _ in 0..ch_count {
                planes.push(c.take(pixels)?.to_vec());
            }
        }
        1 => {
            let table_entries = ch_count
                .checked_mul(h as usize)
                .ok_or_else(|| "PSD merged RLE table overflow".to_string())?;
            let mut row_lens = Vec::with_capacity(table_entries);
            for _ in 0..table_entries {
                row_lens.push(c.read_u16_be()? as usize);
            }
            let stride = w as usize;
            for ch in 0..ch_count {
                let mut plane = vec![0u8; pixels];
                for r in 0..h as usize {
                    let rlen = row_lens.get(ch * (h as usize) + r).copied().unwrap_or(0);
                    let row_src = c.take(rlen)?;
                    let start = r * stride;
                    let dst = plane
                        .get_mut(start..start + stride)
                        .ok_or_else(|| "PSD merged row out of bounds".to_string())?;
                    decode_packbits_row(row_src, dst)?;
                }
                planes.push(plane);
            }
        }
        other => {
            return Err(format!(
                "unsupported PSD merged compression {other} (only Raw=0 and RLE=1 are supported)"
            ));
        }
    }

    let mut rgba = vec![255u8; pixels * 4];
    if color_mode == 1 {
        let gray = planes.first().map(Vec::as_slice).unwrap_or(&[]);
        let alpha = if ch_count >= 2 { planes.get(1).map(Vec::as_slice) } else { None };
        for i in 0..pixels {
            let g = gray.get(i).copied().unwrap_or(0);
            let a = alpha.and_then(|p| p.get(i).copied()).unwrap_or(255);
            if let Some(dst) = rgba.get_mut(i * 4..i * 4 + 4) {
                dst.copy_from_slice(&[g, g, g, a]);
            }
        }
    } else {
        let r_plane = planes.first().map(Vec::as_slice).unwrap_or(&[]);
        let g_plane = planes.get(1).map(Vec::as_slice).unwrap_or(r_plane);
        let b_plane = planes.get(2).map(Vec::as_slice).unwrap_or(r_plane);
        let a_plane = if ch_count >= 4 { planes.get(3).map(Vec::as_slice) } else { None };
        for i in 0..pixels {
            let r = r_plane.get(i).copied().unwrap_or(0);
            let g = g_plane.get(i).copied().unwrap_or(0);
            let b = b_plane.get(i).copied().unwrap_or(0);
            let a = a_plane.and_then(|p| p.get(i).copied()).unwrap_or(255);
            if let Some(dst) = rgba.get_mut(i * 4..i * 4 + 4) {
                dst.copy_from_slice(&[r, g, b, a]);
            }
        }
    }
    Ok(rgba)
}

#[derive(Clone, Debug)]
struct RawLayerRecord {
    name: String,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    width: u32,
    height: u32,
    channels: Vec<(i16, u32)>,
    blend: BlendMode,
    opacity: u8,
    clipped: bool,
    visible: bool,
    section: PsdSectionKind,
}

fn parse_extra_data(extra_bytes: &[u8]) -> std::result::Result<(String, PsdSectionKind, Option<BlendMode>), String> {
    let mut c = Cursor::new(extra_bytes);
    let mask_len = c.read_u32_be()? as usize;
    let _ = c.take(mask_len)?;
    let ranges_len = c.read_u32_be()? as usize;
    let _ = c.take(ranges_len)?;

    // Pascal string padded so (1 + name_len) is a multiple of 4 bytes.
    let name_len = c.read_u8()? as usize;
    let pascal_bytes = c.take(name_len)?;
    let pad = (4 - ((1 + name_len) % 4)) % 4;
    let _ = c.take(pad)?;
    let mut name: String = pascal_bytes
        .iter()
        .map(|&b| char::from(b))
        .filter(|ch| !ch.is_control())
        .take(128)
        .collect();

    let mut section = PsdSectionKind::Normal;
    let mut section_blend = None;

    // Additional Layer Information blocks (`8BIM` or `8B64`).
    while c.remaining().len() >= 12 {
        // Skip up to 3 alignment zero bytes if the writer padded a previous odd block to 2 or 4 bytes.
        while c.remaining().first() == Some(&0) && !c.remaining().starts_with(b"8BIM") && !c.remaining().starts_with(b"8B64") {
            let _ = c.take(1)?;
        }
        if c.remaining().len() < 12 {
            break;
        }
        let sig = c.read_fourcc()?;
        if &sig != b"8BIM" && &sig != b"8B64" {
            break;
        }
        let key = c.read_fourcc()?;
        let block_len = c.read_u32_be()? as usize;
        let block = c.take(block_len)?;
        // PSD aligns odd-length blocks to an even byte boundary.
        if block_len % 2 == 1 && !c.remaining().is_empty() && !c.remaining().starts_with(b"8BIM") && !c.remaining().starts_with(b"8B64") {
            let _ = c.take(1);
        }

        match &key {
            b"luni" if block.len() >= 4 => {
                let mut bc = Cursor::new(block);
                let char_count = (bc.read_u32_be()? as usize).min(MAX_NAME_CODE_UNITS);
                let avail = bc.remaining().len() / 2;
                let n = char_count.min(avail);
                let mut units = Vec::with_capacity(n);
                for _ in 0..n {
                    units.push(bc.read_u16_be()?);
                }
                let decoded: String = char::decode_utf16(units)
                    .map(|r| r.unwrap_or('\u{FFFD}'))
                    .filter(|ch| *ch != '\0' && !ch.is_control())
                    .take(128)
                    .collect();
                if !decoded.is_empty() {
                    name = decoded;
                }
            }
            b"lsct" | b"lsdk" if block.len() >= 4 => {
                let mut bc = Cursor::new(block);
                let divider = bc.read_u32_be()?;
                section = match divider {
                    1 => PsdSectionKind::Folder { open: true },
                    2 => PsdSectionKind::Folder { open: false },
                    3 => PsdSectionKind::FolderEnd,
                    _ => PsdSectionKind::Normal,
                };
                if bc.remaining().len() >= 8 {
                    let bsig = bc.read_fourcc()?;
                    let bkey = bc.read_fourcc()?;
                    if &bsig == b"8BIM" {
                        section_blend = Some(map_blend_mode(&bkey));
                    }
                }
            }
            _ => {}
        }
    }

    Ok((name, section, section_blend))
}

/// Parses a `.psd` v1 byte slice into [`PsdDocument`].
pub fn parse_psd(bytes: &[u8]) -> std::result::Result<PsdDocument, String> {
    if bytes.len() as u64 > MAX_PSD_BYTES {
        return Err(format!("PSD exceeds maximum size of {} MiB", MAX_PSD_BYTES / (1024 * 1024)));
    }
    let mut c = Cursor::new(bytes);
    let sig = c.read_fourcc()?;
    if &sig != b"8BPS" {
        return Err("not a Photoshop PSD file (missing 8BPS signature)".into());
    }
    let version = c.read_u16_be()?;
    if version != 1 {
        return Err(format!("unsupported PSD version {version} (expected version 1 .psd)"));
    }
    let _reserved = c.take(6)?;
    let channels = c.read_u16_be()?;
    if !(1..=MAX_CHANNELS as u16).contains(&channels) {
        return Err(format!("unsupported PSD channel count {channels}"));
    }
    let height = c.read_u32_be()?;
    let width = c.read_u32_be()?;
    if width == 0 || height == 0 || width > MAX_BITMAP_EDGE || height > MAX_BITMAP_EDGE {
        return Err(format!("PSD dimensions {width}×{height} must be in 1..={MAX_BITMAP_EDGE}"));
    }
    let depth = c.read_u16_be()?;
    if depth != 8 {
        return Err(format!("unsupported PSD bit depth {depth} (expected 8-bit channels)"));
    }
    let color_mode = c.read_u16_be()?;
    if color_mode != 1 && color_mode != 3 {
        return Err(format!("unsupported PSD colour mode {color_mode} (expected RGB=3 or Grayscale=1)"));
    }

    // Color Mode Data & Image Resources.
    let color_data_len = c.read_u32_be()? as usize;
    let _ = c.take(color_data_len)?;
    let resources_len = c.read_u32_be()? as usize;
    let _ = c.take(resources_len)?;

    // Layer and Mask Information.
    let lmi_len = c.read_u32_be()? as usize;
    let lmi_bytes = c.take(lmi_len)?;

    let mut layers = Vec::new();
    if lmi_bytes.len() >= 4 {
        let mut lmi = Cursor::new(lmi_bytes);
        let layer_info_len = lmi.read_u32_be()? as usize;
        if layer_info_len >= 2 {
            let li_bytes = lmi.take(layer_info_len)?;
            let mut li = Cursor::new(li_bytes);
            let raw_count = li.read_i16_be()?;
            let layer_count = raw_count.unsigned_abs() as usize;
            if layer_count > MAX_PSD_LAYERS {
                return Err(format!("PSD has {layer_count} layers (limit is {MAX_PSD_LAYERS})"));
            }

            let mut records = Vec::with_capacity(layer_count);
            for idx in 0..layer_count {
                let top = li.read_i32_be()?;
                let left = li.read_i32_be()?;
                let bottom = li.read_i32_be()?;
                let right = li.read_i32_be()?;
                if bottom < top || right < left {
                    return Err(format!("PSD layer {} has inverted bounds ({left},{top})..({right},{bottom})", idx + 1));
                }
                let lw_i64 = i64::from(right) - i64::from(left);
                let lh_i64 = i64::from(bottom) - i64::from(top);
                if lw_i64 > i64::from(MAX_BITMAP_EDGE) || lh_i64 > i64::from(MAX_BITMAP_EDGE) {
                    return Err(format!("PSD layer {} size {lw_i64}×{lh_i64} exceeds {MAX_BITMAP_EDGE}", idx + 1));
                }
                let (lw, lh) = (lw_i64 as u32, lh_i64 as u32);
                let ch_num = li.read_u16_be()? as usize;
                if ch_num > MAX_CHANNELS {
                    return Err(format!("PSD layer {} has too many channels ({ch_num})", idx + 1));
                }
                let mut ch_specs = Vec::with_capacity(ch_num);
                for _ in 0..ch_num {
                    let cid = li.read_i16_be()?;
                    let clen = li.read_u32_be()?;
                    ch_specs.push((cid, clen));
                }
                let bsig = li.read_fourcc()?;
                if &bsig != b"8BIM" {
                    return Err(format!("PSD layer {} has invalid blend signature", idx + 1));
                }
                let bkey = li.read_fourcc()?;
                let mut blend = map_blend_mode(&bkey);
                let opacity = li.read_u8()?;
                let clipping = li.read_u8()?;
                let flags = li.read_u8()?;
                let _filler = li.read_u8()?;
                let extra_len = li.read_u32_be()? as usize;
                let extra = li.take(extra_len)?;
                let (mut name, section, sec_blend) = parse_extra_data(extra)?;
                if let Some(sb) = sec_blend {
                    blend = sb;
                }
                if name.trim().is_empty() {
                    name = format!("Layer {}", idx + 1);
                }
                let visible = (flags & 0x02) == 0;
                records.push(RawLayerRecord {
                    name,
                    top,
                    left,
                    bottom,
                    right,
                    width: lw,
                    height: lh,
                    channels: ch_specs,
                    blend,
                    opacity,
                    clipped: clipping != 0,
                    visible,
                    section,
                });
            }

            // Channel Image Data for each layer record, in order.
            for rec in records {
                let pixels = (rec.width as usize).saturating_mul(rec.height as usize);
                let want_pixels = pixels > 0 && rec.section == PsdSectionKind::Normal;
                let mut r_plane: Option<Vec<u8>> = None;
                let mut g_plane: Option<Vec<u8>> = None;
                let mut b_plane: Option<Vec<u8>> = None;
                let mut a_plane: Option<Vec<u8>> = None;

                for &(cid, clen) in &rec.channels {
                    let cbytes = li.take(clen as usize)?;
                    if !want_pixels || !matches!(cid, -1..=2) {
                        continue;
                    }
                    let decoded = decode_channel_plane(cbytes, rec.width, rec.height)?;
                    match cid {
                        0 => r_plane = Some(decoded),
                        1 => g_plane = Some(decoded),
                        2 => b_plane = Some(decoded),
                        -1 => a_plane = Some(decoded),
                        _ => {}
                    }
                }

                let rgba = if want_pixels {
                    let mut buf = vec![0u8; pixels * 4];
                    if color_mode == 1 {
                        let gray = r_plane.as_deref().unwrap_or(&[]);
                        let alpha = a_plane.as_deref();
                        for i in 0..pixels {
                            let g = gray.get(i).copied().unwrap_or(0);
                            let a = alpha.and_then(|p| p.get(i).copied()).unwrap_or(255);
                            if let Some(dst) = buf.get_mut(i * 4..i * 4 + 4) {
                                dst.copy_from_slice(&[g, g, g, a]);
                            }
                        }
                    } else {
                        let rp = r_plane.as_deref().unwrap_or(&[]);
                        let gp = g_plane.as_deref().unwrap_or(rp);
                        let bp = b_plane.as_deref().unwrap_or(rp);
                        let ap = a_plane.as_deref();
                        for i in 0..pixels {
                            let r = rp.get(i).copied().unwrap_or(0);
                            let g = gp.get(i).copied().unwrap_or(0);
                            let b = bp.get(i).copied().unwrap_or(0);
                            let a = ap.and_then(|p| p.get(i).copied()).unwrap_or(255);
                            if let Some(dst) = buf.get_mut(i * 4..i * 4 + 4) {
                                dst.copy_from_slice(&[r, g, b, a]);
                            }
                        }
                    }
                    buf
                } else {
                    Vec::new()
                };

                layers.push(PsdLayer {
                    name: rec.name,
                    top: rec.top,
                    left: rec.left,
                    bottom: rec.bottom,
                    right: rec.right,
                    width: rec.width,
                    height: rec.height,
                    opacity: rec.opacity,
                    visible: rec.visible,
                    clipped: rec.clipped,
                    blend: rec.blend,
                    section: rec.section,
                    rgba,
                });
            }
        }
    }

    // Decode merged image data when present (required fallback for 0-layer PSDs and flattened mode).
    let merged_rgba = if !c.remaining().is_empty() {
        decode_merged_section(&mut c, width, height, channels, color_mode).ok()
    } else {
        None
    };

    if layers.is_empty() && merged_rgba.is_none() {
        return Err("PSD contains neither layers nor merged image data".into());
    }

    Ok(PsdDocument { width, height, channels, color_mode, layers, merged_rgba })
}

fn read_psd_file(path: &Path) -> Result<PsdDocument> {
    if crate::vfs::len(path)? > MAX_PSD_BYTES {
        return Err(EngineError::Format(format!("{} is too large to import", path.display())));
    }
    let bytes = crate::vfs::read(path)?;
    parse_psd(&bytes).map_err(|e| EngineError::Format(format!("{}: {e}", path.display())))
}

/// Composites parsed PSD layers into a single straight-RGBA8 buffer when a PSD lacks merged data
/// in `"flattened"` mode.
fn composite_layers_rgba(psd: &PsdDocument) -> Vec<u8> {
    if let Some(merged) = &psd.merged_rgba {
        return merged.clone();
    }
    let (w, h) = (psd.width as usize, psd.height as usize);
    let mut dst = vec![0u8; w * h * 4];
    for layer in &psd.layers {
        if !layer.visible || layer.section != PsdSectionKind::Normal || layer.rgba.is_empty() {
            continue;
        }
        let op = f32::from(layer.opacity) / 255.0;
        let lw = layer.width as usize;
        for ly in 0..layer.height as usize {
            let dy = layer.top + ly as i32;
            if dy < 0 || dy >= psd.height as i32 {
                continue;
            }
            for lx in 0..lw {
                let dx = layer.left + lx as i32;
                if dx < 0 || dx >= psd.width as i32 {
                    continue;
                }
                let si = (ly * lw + lx) * 4;
                let di = (dy as usize * w + dx as usize) * 4;
                let (Some(sp), Some(dp)) = (layer.rgba.get(si..si + 4), dst.get_mut(di..di + 4)) else { continue };
                let sa = (f32::from(sp[3]) / 255.0) * op;
                if sa <= 0.0 {
                    continue;
                }
                let da = f32::from(dp[3]) / 255.0;
                let out_a = sa + da * (1.0 - sa);
                if out_a > 1e-6 {
                    for ch in 0..3 {
                        let sc = f32::from(sp[ch]) / 255.0;
                        let dc = f32::from(dp[ch]) / 255.0;
                        let c = (sc * sa + dc * da * (1.0 - sa)) / out_a;
                        dp[ch] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                    dp[3] = (out_a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }
    dst
}

/// Builds an element for one PSD pixel layer (wrapping in a `MovieClip` symbol when `blend != Normal`).
fn build_layer_element(
    doc: &mut Document,
    pl: &PsdLayer,
    offset_x: f32,
    offset_y: f32,
) -> Option<(Arc<Element>, BitmapId)> {
    if pl.width == 0 || pl.height == 0 || pl.rgba.is_empty() {
        return None;
    }
    let bid = BitmapId(doc.alloc_id());
    let bmp_name = doc.unique_name(None, &pl.name);
    let bmp = Bitmap::from_rgba(bid, bmp_name, pl.width, pl.height, pl.rgba.clone())?;
    doc.bitmaps.push(Arc::new(bmp));

    let (w, h) = (pl.width as f32, pl.height as f32);
    let shape = Shape {
        path: Arc::new(GeomPath::rect(0.0, 0.0, w, h)),
        fill: Some(Paint::Bitmap { bitmap: bid, repeat: false, matrix: Affine::IDENTITY }),
        stroke: None,
        fill_rule: FillRule::NonZero,
        merge: false,
        warp: None,
        fill_tag: None,
        stroke_tag: None,
    };
    let tx = offset_x + pl.left as f32;
    let ty = offset_y + pl.top as f32;

    if pl.blend.is_normal() {
        let el = Element {
            id: ElementId(doc.alloc_id()),
            name: String::new(),
            transform: Affine::translate(tx, ty),
            color: ColorEffect::None,
            transform_3d: None,
            kind: ElementKind::Shape(shape),
        };
        Some((Arc::new(el), bid))
    } else {
        // Wrap non-Normal blend modes in a MovieClip symbol instance so both CPU oracle and GPU
        // stage composite the exact Photoshop blend mode.
        let inner_el = Element {
            id: ElementId(doc.alloc_id()),
            name: String::new(),
            transform: Affine::IDENTITY,
            color: ColorEffect::None,
            transform_3d: None,
            kind: ElementKind::Shape(shape),
        };
        let mut inner_layer = Layer::new(LayerId(doc.alloc_id()), "Bitmap");
        if let Some(k) = inner_layer.keyframe_mut(0) {
            k.elements.push(Arc::new(inner_el));
        }
        let sym_id = SymbolId(doc.alloc_id());
        let sym_name = doc.unique_name(None, &format!("{} ({})", pl.name, pl.blend.label()));
        doc.library.push(Arc::new(Symbol {
            id: sym_id,
            name: sym_name,
            kind: SymbolKind::MovieClip,
            linkage: None,
            scale_grid: None,
            scale_grid_kept: None,
            timeline: Timeline { layers: vec![Arc::new(inner_layer)] },
        }));
        let inst = Element {
            id: ElementId(doc.alloc_id()),
            name: String::new(),
            transform: Affine::translate(tx, ty),
            color: ColorEffect::None,
            transform_3d: None,
            kind: ElementKind::Symbol {
                symbol: sym_id,
                loop_mode: LoopMode::Loop,
                first_frame: 0,
                blend: pl.blend,
                filters: Vec::new(),
                events: Vec::new(),
                pivot: None,
            },
        };
        Some((Arc::new(inst), bid))
    }
}

/// Converts a parsed `PsdDocument`'s layers (top-to-bottom stack order) into AniCraft [`Layer`]s.
#[allow(clippy::too_many_arguments)]
fn build_timeline_layers(
    doc: &mut Document,
    psd: &PsdDocument,
    offset_x: f32,
    offset_y: f32,
    frame: u32,
    end_frame: u32,
    root_parent: Option<LayerId>,
    base_color_idx: usize,
) -> (Vec<Arc<Layer>>, Vec<ElementId>, Vec<BitmapId>, usize) {
    let palette = [0x4fa3ff, 0xff6b4a, 0x66bb6a, 0xffd54a, 0xba68c8, 0x4dd0e1, 0xff8a65];
    let mut color_idx = base_color_idx;
    let mut out_layers: Vec<Arc<Layer>> = Vec::new();
    let mut element_ids = Vec::new();
    let mut bitmap_ids = Vec::new();
    let mut folder_count = 0usize;
    let mut folder_stack: Vec<LayerId> = Vec::new();

    let make_keys = |els: Vec<Arc<Element>>| -> Vec<Arc<Keyframe>> {
        let dur = end_frame.saturating_sub(frame).max(1);
        let mut kf = Keyframe::new(frame, dur);
        kf.elements = els;
        if frame == 0 {
            vec![Arc::new(kf)]
        } else {
            vec![Arc::new(Keyframe::new(0, frame)), Arc::new(kf)]
        }
    };

    // PSD stores layer records bottom-to-top; AniCraft timelines store top-to-bottom (index 0 = top).
    for pl in psd.layers.iter().rev() {
        let current_parent = folder_stack.last().copied().or(root_parent);
        match pl.section {
            PsdSectionKind::FolderEnd => {
                let _ = folder_stack.pop();
            }
            PsdSectionKind::Folder { .. } => {
                let fid = LayerId(doc.alloc_id());
                let mut folder = Layer::new(fid, pl.name.clone());
                folder.kind = LayerKind::Folder;
                folder.parent = current_parent;
                folder.visible = pl.visible;
                folder.opacity = (f32::from(pl.opacity) / 255.0).clamp(0.0, 1.0);
                color_idx += 1;
                folder.color = Rgba::hex(palette[color_idx % palette.len()]);
                folder.keyframes.clear();
                out_layers.push(Arc::new(folder));
                folder_count += 1;
                if folder_stack.len() + 1 < MAX_LAYER_DEPTH {
                    folder_stack.push(fid);
                }
            }
            PsdSectionKind::Normal => {
                let lid = LayerId(doc.alloc_id());
                let mut layer = Layer::new(lid, pl.name.clone());
                layer.kind = LayerKind::Normal;
                layer.parent = current_parent;
                layer.visible = pl.visible;
                layer.opacity = (f32::from(pl.opacity) / 255.0).clamp(0.0, 1.0);
                color_idx += 1;
                layer.color = Rgba::hex(palette[color_idx % palette.len()]);
                let mut els = Vec::new();
                if let Some((el, bid)) = build_layer_element(doc, pl, offset_x, offset_y) {
                    element_ids.push(el.id);
                    bitmap_ids.push(bid);
                    els.push(el);
                }
                layer.keyframes = make_keys(els);
                layer.normalize();
                out_layers.push(Arc::new(layer));
            }
        }
    }

    (out_layers, element_ids, bitmap_ids, folder_count)
}

/// `file.import_psd` implementation (also called by `file.import` on `.psd` files).
pub(crate) fn import_psd_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let path = PathBuf::from(str_req(p, "path")?);
    let flattened = match opt_str(p, "mode")? {
        None | Some("layers") => false,
        Some("flattened") => true,
        Some(other) => {
            return Err(EngineError::Param(format!(
                "param 'mode' must be 'layers' or 'flattened', not '{other}'"
            )));
        }
    };
    let psd = read_psd_file(&path)?;
    let stage = s.doc.stage;
    let ox = match opt_f32(p, "cx")? {
        Some(cx) => cx - psd.width as f32 * 0.5,
        None => opt_f32(p, "x")?.unwrap_or((stage.width as f32 - psd.width as f32) * 0.5),
    };
    let oy = match opt_f32(p, "cy")? {
        Some(cy) => cy - psd.height as f32 * 0.5,
        None => opt_f32(p, "y")?.unwrap_or((stage.height as f32 - psd.height as f32) * 0.5),
    };
    if !(ox.is_finite() && oy.is_finite()) || ox.abs() > 1.0e6 || oy.abs() > 1.0e6 {
        return Err(EngineError::Param("'x' and 'y' must be finite numbers within ±1000000".into()));
    }

    let stem: String = path
        .file_stem()
        .map_or_else(|| "Photoshop".into(), |n| n.to_string_lossy().chars().take(128).collect());
    let frame = s.frame;
    let end = s.frame_count().max(frame + 1);

    // Flattened mode or 0-layer PSD: fall back to merged image data on one layer.
    let has_normal_layers = psd.layers.iter().any(|l| l.section == PsdSectionKind::Normal);
    if flattened || !has_normal_layers {
        let rgba = composite_layers_rgba(&psd);
        let bid = BitmapId(s.doc.alloc_id());
        let bname = s.doc.unique_name(None, &stem);
        let bmp = Bitmap::from_rgba(bid, bname, psd.width, psd.height, rgba)
            .ok_or_else(|| EngineError::Format(format!("{}: could not encode PSD bitmap", path.display())))?;
        s.doc.bitmaps.push(Arc::new(bmp));
        crate::cmds_timeline::new_layer(s, &json!({"name": stem}))?;
        let shape = Shape {
            path: Arc::new(GeomPath::rect(0.0, 0.0, psd.width as f32, psd.height as f32)),
            fill: Some(Paint::Bitmap { bitmap: bid, repeat: false, matrix: Affine::IDENTITY }),
            stroke: None,
            fill_rule: FillRule::NonZero,
            merge: false,
            warp: None,
        fill_tag: None,
        stroke_tag: None,
        };
        let eid = ElementId(s.doc.alloc_id());
        let el = Arc::new(Element {
            id: eid,
            name: String::new(),
            transform: Affine::translate(ox, oy),
            color: ColorEffect::None,
            transform_3d: None,
            kind: ElementKind::Shape(shape),
        });
        let dur = end.saturating_sub(frame).max(1);
        let mut kf = Keyframe::new(frame, dur);
        kf.elements = vec![el];
        let layer = s.layer_mut(None)?;
        layer.keyframes = if frame == 0 {
            vec![Arc::new(kf)]
        } else {
            vec![Arc::new(Keyframe::new(0, frame)), Arc::new(kf)]
        };
        layer.normalize();
        let lid = layer.id;
        s.select_only([eid]);
        return Ok(json!({
            "mode": "flattened",
            "layer": lid.0,
            "layers": 1,
            "folders": 0,
            "elements": 1,
            "bitmap": bid.0,
            "bitmaps": [bid.0],
            "width": psd.width,
            "height": psd.height,
            "report": "Imported flattened Photoshop image.",
        }));
    }

    let existing_len = s.timeline().layers.len();
    let mut insert_at = s.active_layer_index().unwrap_or(0);
    let mut parent = s.timeline().layers.get(insert_at).and_then(|l| l.parent);
    if let Some(m) = parent.and_then(|pid| s.timeline().layer(pid)).filter(|m| m.kind == LayerKind::Mask) {
        parent = m.parent;
        insert_at = s.timeline().layers.iter().position(|l| l.id == m.id).unwrap_or(insert_at);
    }

    let (built_layers, element_ids, bitmap_ids, folders) =
        build_timeline_layers(&mut s.doc, &psd, ox, oy, frame, end, parent, existing_len);

    if existing_len + built_layers.len() > 1000 {
        return Err(EngineError::State(format!(
            "importing {} would exceed the 1000-layer limit",
            path.display()
        )));
    }

    let made = built_layers.len();
    let first_drawable = built_layers
        .iter()
        .find(|l| l.kind == LayerKind::Normal)
        .or_else(|| built_layers.first())
        .map(|l| l.id);

    let tl = crate::editing::edited_mut(&mut s.doc, s.scene, &s.editing)?;
    for (k, l) in built_layers.into_iter().enumerate() {
        tl.layers.insert((insert_at + k).min(tl.layers.len()), l);
    }
    tl.repair_masks();

    if let Some(lid) = first_drawable {
        s.layer = lid;
    }
    if let Some(top_normal) = s.timeline().layer(s.layer) {
        let top_ids: Vec<ElementId> = top_normal
            .keyframe_at(frame)
            .map(|k| k.elements.iter().map(|e| e.id).collect())
            .unwrap_or_default();
        s.select_only(top_ids);
    }

    Ok(json!({
        "mode": "layers",
        "layer": s.layer.0,
        "layers": made,
        "folders": folders,
        "elements": element_ids.len(),
        "bitmap": bitmap_ids.first().map(|b| b.0),
        "bitmaps": bitmap_ids.iter().map(|b| b.0).collect::<Vec<_>>(),
        "width": psd.width,
        "height": psd.height,
        "report": format!("Imported {made} Photoshop layers ({} bitmaps).", bitmap_ids.len()),
    }))
}

/// Opens a `.psd` file as a fresh untitled AniCraft document sized to the PSD canvas (`file.open`).
pub(crate) fn open_psd(s: &mut Session, path: &Path) -> Result<Value> {
    let psd = read_psd_file(path)?;
    let mut doc = Document::default();
    doc.stage.width = psd.width.clamp(1, anicraft_doc::MAX_STAGE_EDGE);
    doc.stage.height = psd.height.clamp(1, anicraft_doc::MAX_STAGE_EDGE);

    let has_normal_layers = psd.layers.iter().any(|l| l.section == PsdSectionKind::Normal);
    let (layers, bitmaps_count) = if has_normal_layers {
        let (built, _, bids, _) = build_timeline_layers(&mut doc, &psd, 0.0, 0.0, 0, 1, None, 0);
        (built, bids.len())
    } else {
        let rgba = composite_layers_rgba(&psd);
        let stem: String = path
            .file_stem()
            .map_or_else(|| "Background".into(), |n| n.to_string_lossy().chars().take(128).collect());
        let bid = BitmapId(doc.alloc_id());
        let bmp = Bitmap::from_rgba(bid, stem.clone(), psd.width, psd.height, rgba)
            .ok_or_else(|| EngineError::Format(format!("{}: could not encode PSD bitmap", path.display())))?;
        doc.bitmaps.push(Arc::new(bmp));
        let shape = Shape {
            path: Arc::new(GeomPath::rect(0.0, 0.0, psd.width as f32, psd.height as f32)),
            fill: Some(Paint::Bitmap { bitmap: bid, repeat: false, matrix: Affine::IDENTITY }),
            stroke: None,
            fill_rule: FillRule::NonZero,
            merge: false,
            warp: None,
        fill_tag: None,
        stroke_tag: None,
        };
        let mut layer = Layer::new(LayerId(doc.alloc_id()), stem);
        if let Some(k) = layer.keyframe_mut(0) {
            k.elements.push(Arc::new(Element {
                id: ElementId(doc.alloc_id()),
                name: String::new(),
                transform: Affine::IDENTITY,
                color: ColorEffect::None,
                transform_3d: None,
                kind: ElementKind::Shape(shape),
            }));
        }
        (vec![Arc::new(layer)], 1)
    };

    if !layers.is_empty() {
        let tl = doc.timeline_mut(0);
        tl.layers = layers;
        tl.repair_masks();
    }

    let layer_count = doc.timeline(0).layers.len();
    s.replace_document(doc, None);
    s.recent.add(path);
    let mut info = s.info();
    info["imported"] = json!(path.display().to_string());
    info["report"] = json!(format!("Imported Photoshop document ({layer_count} layers, {bitmaps_count} bitmaps)."));
    Ok(info)
}

/// Imports a `.psd` into the Library as a Graphic Symbol containing all of its layers, centred on
/// its registration point (used by `file.import_to_library` and `file.drop` onto Library/Timeline).
pub(crate) fn import_psd_symbol(s: &mut Session, path: &Path, name: Option<&str>) -> Result<Value> {
    let psd = read_psd_file(path)?;
    let stem: String = path
        .file_stem()
        .map_or_else(|| "Photoshop".into(), |n| n.to_string_lossy().chars().take(128).collect());
    let sym_name = s.doc.unique_name(None, name.map_or(stem.as_str(), str::trim));
    let ox = -(psd.width as f32 * 0.5);
    let oy = -(psd.height as f32 * 0.5);

    let has_normal_layers = psd.layers.iter().any(|l| l.section == PsdSectionKind::Normal);
    let (layers, count) = if has_normal_layers {
        let (built, eids, _, _) = build_timeline_layers(&mut s.doc, &psd, ox, oy, 0, 1, None, 0);
        (built, eids.len())
    } else {
        let rgba = composite_layers_rgba(&psd);
        let bid = BitmapId(s.doc.alloc_id());
        let bname = s.doc.unique_name(None, &stem);
        let bmp = Bitmap::from_rgba(bid, bname, psd.width, psd.height, rgba)
            .ok_or_else(|| EngineError::Format(format!("{}: could not encode PSD bitmap", path.display())))?;
        s.doc.bitmaps.push(Arc::new(bmp));
        let shape = Shape {
            path: Arc::new(GeomPath::rect(0.0, 0.0, psd.width as f32, psd.height as f32)),
            fill: Some(Paint::Bitmap { bitmap: bid, repeat: false, matrix: Affine::IDENTITY }),
            stroke: None,
            fill_rule: FillRule::NonZero,
            merge: false,
            warp: None,
        fill_tag: None,
        stroke_tag: None,
        };
        let mut layer = Layer::new(LayerId(s.doc.alloc_id()), "Layer 1");
        if let Some(k) = layer.keyframe_mut(0) {
            k.elements.push(Arc::new(Element {
                id: ElementId(s.doc.alloc_id()),
                name: String::new(),
                transform: Affine::translate(ox, oy),
                color: ColorEffect::None,
                transform_3d: None,
                kind: ElementKind::Shape(shape),
            }));
        }
        (vec![Arc::new(layer)], 1)
    };

    let mut timeline = Timeline { layers };
    timeline.repair_masks();
    let layer_len = timeline.layers.len();
    let id = SymbolId(s.doc.alloc_id());
    s.doc.library.push(Arc::new(Symbol {
        id,
        name: sym_name.clone(),
        kind: SymbolKind::Graphic,
        linkage: None,
        scale_grid: None,
        scale_grid_kept: None,
        timeline,
    }));
    Ok(json!({
        "symbol": id.0,
        "name": sym_name,
        "elements": count,
        "layers": layer_len,
        "width": psd.width,
        "height": psd.height,
        "report": format!("Imported Photoshop symbol ({layer_len} layers)."),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PackBits encoder for synthetic RLE test fixtures.
    fn packbits_encode_row(row: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < row.len() {
            // Check for a repeat run of >= 3 identical bytes.
            let mut run = 1usize;
            while i + run < row.len() && run < 128 && row[i + run] == row[i] {
                run += 1;
            }
            if run >= 3 {
                out.push((-(run as i8 - 1)) as u8);
                out.push(row[i]);
                i += run;
            } else {
                let start = i;
                let mut lit = 0usize;
                while i < row.len() && lit < 128 {
                    let mut r = 1usize;
                    while i + r < row.len() && r < 3 && row[i + r] == row[i] {
                        r += 1;
                    }
                    if r >= 3 {
                        break;
                    }
                    i += 1;
                    lit += 1;
                }
                out.push((lit - 1) as u8);
                out.extend_from_slice(&row[start..start + lit]);
            }
        }
        out
    }

    fn encode_rle_plane(w: u32, h: u32, plane: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u16.to_be_bytes()); // Compression = 1 (RLE)
        let mut rows = Vec::new();
        for y in 0..h as usize {
            let start = y * w as usize;
            rows.push(packbits_encode_row(&plane[start..start + w as usize]));
        }
        for r in &rows {
            out.extend_from_slice(&(r.len() as u16).to_be_bytes());
        }
        for r in rows {
            out.extend_from_slice(&r);
        }
        out
    }

    #[derive(Clone)]
    struct SynthLayer {
        pascal_name: &'static str,
        luni_name: Option<&'static str>,
        top: i32,
        left: i32,
        w: u32,
        h: u32,
        blend: [u8; 4],
        opacity: u8,
        visible: bool,
        lsct: Option<u32>,
        rgba: [u8; 4],
        rle: bool,
    }

    fn build_psd_fixture(doc_w: u32, doc_h: u32, layers: &[SynthLayer]) -> Vec<u8> {
        let mut psd = Vec::new();
        // Header (26 bytes)
        psd.extend_from_slice(b"8BPS");
        psd.extend_from_slice(&1u16.to_be_bytes());
        psd.extend_from_slice(&[0u8; 6]);
        psd.extend_from_slice(&4u16.to_be_bytes()); // RGBA merged
        psd.extend_from_slice(&doc_h.to_be_bytes());
        psd.extend_from_slice(&doc_w.to_be_bytes());
        psd.extend_from_slice(&8u16.to_be_bytes()); // 8-bit
        psd.extend_from_slice(&3u16.to_be_bytes()); // RGB mode

        // Color Mode Data (0) & Image Resources (0)
        psd.extend_from_slice(&0u32.to_be_bytes());
        psd.extend_from_slice(&0u32.to_be_bytes());

        // Layer Info
        if layers.is_empty() {
            psd.extend_from_slice(&0u32.to_be_bytes());
        } else {
            let mut li = Vec::new();
            li.extend_from_slice(&(layers.len() as i16).to_be_bytes());

            let mut all_channel_blobs: Vec<Vec<Vec<u8>>> = Vec::new();
            for l in layers {
                li.extend_from_slice(&l.top.to_be_bytes());
                li.extend_from_slice(&l.left.to_be_bytes());
                li.extend_from_slice(&(l.top + l.h as i32).to_be_bytes());
                li.extend_from_slice(&(l.left + l.w as i32).to_be_bytes());
                li.extend_from_slice(&4u16.to_be_bytes()); // -1 (A), 0 (R), 1 (G), 2 (B)

                let px = (l.w * l.h) as usize;
                let mut blobs = Vec::new();
                for &cid in &[-1i16, 0, 1, 2] {
                    let sample = match cid {
                        0 => l.rgba[0],
                        1 => l.rgba[1],
                        2 => l.rgba[2],
                        _ => l.rgba[3],
                    };
                    let blob = if px == 0 {
                        0u16.to_be_bytes().to_vec()
                    } else if l.rle {
                        encode_rle_plane(l.w, l.h, &vec![sample; px])
                    } else {
                        let mut raw = 0u16.to_be_bytes().to_vec();
                        raw.extend(std::iter::repeat_n(sample, px));
                        raw
                    };
                    li.extend_from_slice(&cid.to_be_bytes());
                    li.extend_from_slice(&(blob.len() as u32).to_be_bytes());
                    blobs.push(blob);
                }
                all_channel_blobs.push(blobs);

                li.extend_from_slice(b"8BIM");
                li.extend_from_slice(&l.blend);
                li.push(l.opacity);
                li.push(0); // clipping
                li.push(if l.visible { 0 } else { 0x02 }); // bit 1 = hidden
                li.push(0);

                // Extra data
                let mut extra = Vec::new();
                extra.extend_from_slice(&0u32.to_be_bytes()); // mask
                extra.extend_from_slice(&0u32.to_be_bytes()); // blending ranges
                let pbytes = l.pascal_name.as_bytes();
                extra.push(pbytes.len() as u8);
                extra.extend_from_slice(pbytes);
                let pad = (4 - ((1 + pbytes.len()) % 4)) % 4;
                extra.extend(std::iter::repeat_n(0u8, pad));

                if let Some(uni) = l.luni_name {
                    let units: Vec<u16> = uni.encode_utf16().collect();
                    let mut block = Vec::new();
                    block.extend_from_slice(&(units.len() as u32).to_be_bytes());
                    for u in units {
                        block.extend_from_slice(&u.to_be_bytes());
                    }
                    if block.len() % 2 == 1 {
                        block.push(0);
                    }
                    extra.extend_from_slice(b"8BIM");
                    extra.extend_from_slice(b"luni");
                    extra.extend_from_slice(&(block.len() as u32).to_be_bytes());
                    extra.extend_from_slice(&block);
                }
                if let Some(div) = l.lsct {
                    extra.extend_from_slice(b"8BIM");
                    extra.extend_from_slice(b"lsct");
                    extra.extend_from_slice(&4u32.to_be_bytes());
                    extra.extend_from_slice(&div.to_be_bytes());
                }

                li.extend_from_slice(&(extra.len() as u32).to_be_bytes());
                li.extend_from_slice(&extra);
            }

            for blobs in all_channel_blobs {
                for b in blobs {
                    li.extend_from_slice(&b);
                }
            }
            if li.len() % 2 == 1 {
                li.push(0);
            }
            let mut lmi = Vec::new();
            lmi.extend_from_slice(&(li.len() as u32).to_be_bytes());
            lmi.extend_from_slice(&li);
            psd.extend_from_slice(&(lmi.len() as u32).to_be_bytes());
            psd.extend_from_slice(&lmi);
        }

        // Merged Image Data (RLE compressed solid magenta-ish backdrop for 0-layer fallback check)
        psd.extend_from_slice(&1u16.to_be_bytes());
        let mut merged_rows = Vec::new();
        for ch in 0..4usize {
            let val = [40u8, 80, 200, 255][ch];
            let row = packbits_encode_row(&vec![val; doc_w as usize]);
            for _ in 0..doc_h {
                psd.extend_from_slice(&(row.len() as u16).to_be_bytes());
                merged_rows.push(row.clone());
            }
        }
        for r in merged_rows {
            psd.extend_from_slice(&r);
        }
        psd
    }

    #[test]
    fn imports_rle_multilayer_psd_with_folders_luni_utf16_blends_and_alpha() {
        // Bottom-to-top PSD order:
        // 0: Background (Normal, Raw, opaque blue)
        // 1: </Layer group> (lsct=3, end of Character folder)
        // 2: Multiply Shadow inside folder (mul , RLE, 50% opacity, semi-transparent)
        // 3: Hero Layer with UTF-16 luni name inside folder (norm, RLE, hidden=false)
        // 4: Character Folder header (lsct=1 open folder, UTF-16 luni "الشخصية ✨")
        let fixture = build_psd_fixture(
            64,
            48,
            &[
                SynthLayer {
                    pascal_name: "BG",
                    luni_name: None,
                    top: 0,
                    left: 0,
                    w: 64,
                    h: 48,
                    blend: *b"norm",
                    opacity: 255,
                    visible: true,
                    lsct: None,
                    rgba: [20, 40, 220, 255],
                    rle: false,
                },
                SynthLayer {
                    pascal_name: "</Layer group>",
                    luni_name: None,
                    top: 0,
                    left: 0,
                    w: 0,
                    h: 0,
                    blend: *b"norm",
                    opacity: 255,
                    visible: true,
                    lsct: Some(3),
                    rgba: [0, 0, 0, 0],
                    rle: true,
                },
                SynthLayer {
                    pascal_name: "Shadow",
                    luni_name: Some("Rim Glow"),
                    top: 8,
                    left: 12,
                    w: 24,
                    h: 20,
                    blend: *b"mul ",
                    opacity: 128,
                    visible: true,
                    lsct: None,
                    rgba: [255, 60, 30, 200],
                    rle: true,
                },
                SynthLayer {
                    pascal_name: "HiddenSketch",
                    luni_name: Some("Rough Sketch"),
                    top: 4,
                    left: 4,
                    w: 16,
                    h: 16,
                    blend: *b"hard",
                    opacity: 200,
                    visible: false,
                    lsct: None,
                    rgba: [10, 200, 50, 180],
                    rle: true,
                },
                SynthLayer {
                    pascal_name: "Group",
                    luni_name: Some("الشخصية ✨"),
                    top: 0,
                    left: 0,
                    w: 0,
                    h: 0,
                    blend: *b"pass",
                    opacity: 255,
                    visible: true,
                    lsct: Some(1),
                    rgba: [0, 0, 0, 0],
                    rle: true,
                },
            ],
        );

        let dir = std::env::temp_dir().join(format!("anicraft-psd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scene.psd");
        std::fs::write(&path, &fixture).unwrap();

        let mut s = Session::default();
        let res = s
            .execute("file.import_psd", &json!({"path": path.display().to_string(), "x": 10.0, "y": 20.0}))
            .expect("PSD imports cleanly");
        assert_eq!(res["layers"], 4, "1 folder + 3 artwork layers (</Layer group> skipped) + original default layer stays below");
        assert_eq!(res["folders"], 1);
        assert_eq!(s.doc.bitmaps.len(), 3);

        let tl = s.timeline();
        // Top-to-bottom: [0] folder "الشخصية ✨", [1] "Rough Sketch" (hidden, HardLight MovieClip),
        // [2] "Rim Glow" (Multiply MovieClip, ~50% opacity), [3] "BG" (Normal shape)
        assert_eq!(tl.layers[0].kind, LayerKind::Folder);
        assert_eq!(tl.layers[0].name, "الشخصية ✨");
        let fid = tl.layers[0].id;

        assert_eq!(tl.layers[1].name, "Rough Sketch");
        assert!(!tl.layers[1].visible);
        assert_eq!(tl.layers[1].parent, Some(fid));

        assert_eq!(tl.layers[2].name, "Rim Glow");
        assert_eq!(tl.layers[2].parent, Some(fid));
        assert!((tl.layers[2].opacity - 128.0 / 255.0).abs() < 0.01);
        match &tl.layers[2].keyframes[0].elements[0].kind {
            ElementKind::Symbol { blend, .. } => assert_eq!(*blend, BlendMode::Multiply),
            other => panic!("expected MovieClip wrapper for Multiply blend, got {other:?}"),
        }
        let pos = tl.layers[2].keyframes[0].elements[0].transform.translation();
        assert!((pos.x - 22.0).abs() < 1e-3 && (pos.y - 28.0).abs() < 1e-3);

        assert_eq!(tl.layers[3].name, "BG");
        assert_eq!(tl.layers[3].parent, None);

        // Also test 0-layer PSD fallback to merged image data & file.open hook!
        let flat_psd = build_psd_fixture(32, 24, &[]);
        let flat_path = dir.join("merged_only.psd");
        std::fs::write(&flat_path, &flat_psd).unwrap();
        let mut s2 = Session::default();
        let opened = s2.execute("file.open", &json!({"path": flat_path.display().to_string()})).expect("file.open opens PSD");
        assert_eq!(s2.doc.stage.width, 32);
        assert_eq!(s2.doc.stage.height, 24);
        assert_eq!(s2.doc.bitmaps.len(), 1);
        assert!(opened["imported"].as_str().is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hostile_and_truncated_psd_slices_never_panic() {
        let valid = build_psd_fixture(
            16,
            16,
            &[SynthLayer {
                pascal_name: "A",
                luni_name: Some("Ümlaut"),
                top: 0,
                left: 0,
                w: 16,
                h: 16,
                blend: *b"scrn",
                opacity: 255,
                visible: true,
                lsct: None,
                rgba: [1, 2, 3, 255],
                rle: true,
            }],
        );
        // Every prefix truncation of a valid PSD must return Ok or Err without panicking.
        for cut in (0..valid.len()).step_by(3) {
            let _ = parse_psd(&valid[..cut]);
        }
        // Corrupt header dimensions, layer counts > 512, negative rects, bad RLE runs.
        let mut huge_layers = valid.clone();
        // Set layer_count to 9999 inside LMI if long enough.
        if huge_layers.len() > 40 {
            huge_layers[38] = 0x27;
            huge_layers[39] = 0x0f;
            assert!(parse_psd(&huge_layers).is_err());
        }
        for junk in [
            &b""[..],
            b"NOTPSD",
            b"8BPS\x00\x02\x00\x00\x00\x00\x00\x00\x00\x04\x00\x00\x00\x10\x00\x00\x00\x10\x00\x08\x00\x03",
            b"8BPS\x00\x01\x00\x00\x00\x00\x00\x00\x00\x04\x00\x09\x00\x00\x00\x09\x00\x00\x00\x08\x00\x03",
        ] {
            assert!(parse_psd(junk).is_err());
        }
    }
}
