//! Animate's own media payloads (layer L0): the bitmap and sound data an Animate document keeps
//! for its library items, in the form it writes them.
//!
//! The same bytes appear in two places, so both importers share this crate:
//!
//! * a zip/XFL `.fla` keeps them as `bin/<name>.dat`, named by a `DOMBitmapItem`'s
//!   `bitmapDataHRef` or a `DOMSoundItem`'s `soundDataHRef`;
//! * a legacy (Flash 8–CS4, compound binary) `.fla` keeps them as the streams `Media <n>` or
//!   `M <n> <stamp>`, byte for byte the same as the twin's `bin/*.dat`.
//!
//! Clean room: the layouts below were worked out by observation only, comparing the payloads
//! with the attributes Animate writes beside them in XFL (`frameRight`, `frameBottom`, `format`,
//! `sampleCount`, `dataLength`) and with the images and sounds they decode to. No Animate code
//! or third-party converter was consulted. The zlib stream is the public RFC 1950/1951 format.
//!
//! # Bitmaps ([`decode_bitmap`])
//!
//! Little-endian, from offset 0:
//!
//! | size | field |
//! |---|---|
//! | 1 | `03`, the payload kind |
//! | 1 | pixel format: `05` is 32-bit A,R,G,B (the only one seen) |
//! | 2 | bytes per row (`width × 4` in every sample) |
//! | 2 | width, pixels |
//! | 2 | height, pixels |
//! | 4 × 4 | the frame in twips (i32): left, right, top, bottom |
//! | 1 | has alpha (`00`/`01`) |
//! | 1 | compressed (`01`: zlib follows) |
//! | … | chunks of `u16 length` + that many bytes; a `u16 0` ends them |
//!
//! The chunk bodies joined are one zlib stream that inflates to exactly
//! `row bytes × height`: rows top-down, four bytes a pixel, A, R, G, B, the colour premultiplied
//! by alpha. [`DatBitmap::rgba`] is straight (not premultiplied) RGBA8, as AniCraft's document
//! model keeps pixels.
//!
//! Only format `05`, compressed, has been observed. Other pixel formats, uncompressed rows and
//! JPEG-backed items are refused with [`DatError::Unsupported`] rather than guessed at (a JPEG
//! payload starts `FF D8` and is not this format at all: [`decode_bitmap`] answers
//! [`DatError::NotBitmap`] and a JPEG reader takes it). Without the alpha flag the first byte of
//! a pixel is read as padding and the pixel is opaque, as in SWF's 32-bit lossless bitmaps; this
//! too is unobserved.
//!
//! # Sounds ([`decode_sound`])
//!
//! Raw PCM with no header at all: signed 16-bit little-endian samples, stereo interleaved left,
//! right. The rate and channels are not in the payload; they come from the library item:
//! [`SoundFormat::from_xfl`] reads a `DOMSoundItem`'s `format` (`"44kHz 16bit Stereo"`), and
//! [`SoundFormat::from_legacy`] the format byte of a legacy `CMediaSound` record (`0F` stereo,
//! `0E` mono; read like SWF's sound format bits). Its length is
//! `sampleCount × channels × 2` exactly. [`RawSound::to_wav`] wraps it as a WAV file, losslessly.
//!
//! Compressed (MP3, ADPCM) and 8-bit storage are unobserved and refused.
//!
//! Everything is bounded: images to [`MAX_EDGE`] a side and [`MAX_PIXEL_BYTES`] inflated (the
//! inflater stops at the size the header promises), sounds to [`MAX_SOUND_SAMPLES`]. Nothing
//! here panics, whatever the input.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable, clippy::indexing_slicing)]

/// Largest bitmap edge accepted, pixels (the document model's limit too).
pub const MAX_EDGE: u32 = 8192;
/// Most bytes a bitmap's pixel data may inflate to (`MAX_EDGE² × 4`, 256 MiB).
pub const MAX_PIXEL_BYTES: usize = MAX_EDGE as usize * MAX_EDGE as usize * 4;
/// Most sample values (all channels) a sound may hold: what AniCraft's audio decoder accepts
/// (about 12 minutes of 44.1 kHz stereo).
pub const MAX_SOUND_SAMPLES: u64 = 64 * 1024 * 1024;

/// The first byte of a bitmap payload.
pub const BITMAP_KIND: u8 = 0x03;
/// The pixel format byte of 32-bit A,R,G,B.
pub const FORMAT_ARGB32: u8 = 0x05;
/// Bytes before a bitmap's first chunk.
pub const BITMAP_HEADER_LEN: usize = 26;

/// Why a payload doesn't decode.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DatError {
    /// Doesn't start like an Animate bitmap payload (`03`).
    #[error("not Animate bitmap data")]
    NotBitmap,
    /// Ends before its header or its last chunk does.
    #[error("the data is cut short")]
    Truncated,
    /// A variant that hasn't been observed, so isn't guessed at.
    #[error("{0}, which AniCraft doesn't read yet")]
    Unsupported(String),
    /// A width or height of 0, or over [`MAX_EDGE`]; or rows narrower than their pixels.
    #[error("the image is empty, larger than 8192 px a side, or its rows are too short")]
    BadSize,
    /// Over [`MAX_SOUND_SAMPLES`].
    #[error("longer than AniCraft plays")]
    TooLong,
    /// Inconsistent with itself.
    #[error("damaged data: {0}")]
    Damaged(&'static str),
    /// A sound whose length isn't what its library item says.
    #[error("{found} bytes of sound where its item promises {expected}")]
    LengthMismatch { expected: u64, found: u64 },
    /// A sound of no samples.
    #[error("it holds no sound")]
    Empty,
}

/// A bitmap's frame (its bounds as Animate records them), in twips (1/20 px).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

/// A bitmap payload's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitmapHeader {
    /// The pixel format byte ([`FORMAT_ARGB32`]).
    pub format: u8,
    pub row_bytes: u16,
    pub width: u16,
    pub height: u16,
    pub frame: Frame,
    pub has_alpha: bool,
    pub compressed: bool,
}

/// A decoded bitmap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatBitmap {
    pub width: u32,
    pub height: u32,
    pub frame: Frame,
    /// Whether the payload says the image has transparent pixels.
    pub has_alpha: bool,
    /// `width × height` straight-alpha RGBA8 pixels, rows top-down.
    pub rgba: Vec<u8>,
}

/// Little-endian reads that fail with [`DatError::Truncated`] instead of running off the end.
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DatError> {
        let end = self.at.checked_add(n).ok_or(DatError::Truncated)?;
        let out = self.data.get(self.at..end).ok_or(DatError::Truncated)?;
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, DatError> {
        match *self.take(1)? {
            [b] => Ok(b),
            _ => Err(DatError::Truncated),
        }
    }

    fn u16(&mut self) -> Result<u16, DatError> {
        match *self.take(2)? {
            [a, b] => Ok(u16::from_le_bytes([a, b])),
            _ => Err(DatError::Truncated),
        }
    }

    fn i32(&mut self) -> Result<i32, DatError> {
        match *self.take(4)? {
            [a, b, c, d] => Ok(i32::from_le_bytes([a, b, c, d])),
            _ => Err(DatError::Truncated),
        }
    }

    fn flag(&mut self, what: &str) -> Result<bool, DatError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(DatError::Unsupported(format!("a bitmap {what} flag of {v}"))),
        }
    }
}

/// Whether `data` starts like a bitmap payload (its kind byte, and room for the header), to
/// tell it from other data before decoding.
pub fn is_bitmap(data: &[u8]) -> bool {
    data.first() == Some(&BITMAP_KIND) && data.len() >= BITMAP_HEADER_LEN
}

/// Reads a bitmap payload's header, without its pixels.
pub fn bitmap_header(data: &[u8]) -> Result<BitmapHeader, DatError> {
    read_header(&mut Cursor { data, at: 0 })
}

fn read_header(c: &mut Cursor<'_>) -> Result<BitmapHeader, DatError> {
    if c.u8()? != BITMAP_KIND {
        return Err(DatError::NotBitmap);
    }
    let format = c.u8()?;
    let (row_bytes, width, height) = (c.u16()?, c.u16()?, c.u16()?);
    let frame = Frame { left: c.i32()?, right: c.i32()?, top: c.i32()?, bottom: c.i32()? };
    let has_alpha = c.flag("has-alpha")?;
    let compressed = c.flag("compression")?;
    Ok(BitmapHeader { format, row_bytes, width, height, frame, has_alpha, compressed })
}

/// Decodes a bitmap payload into straight-alpha RGBA8 pixels.
pub fn decode_bitmap(data: &[u8]) -> Result<DatBitmap, DatError> {
    let mut c = Cursor { data, at: 0 };
    let h = read_header(&mut c)?;
    if h.format != FORMAT_ARGB32 {
        return Err(DatError::Unsupported(format!("bitmap pixel format {}", h.format)));
    }
    if !h.compressed {
        return Err(DatError::Unsupported("uncompressed bitmap data".into()));
    }
    let (width, height) = (u32::from(h.width), u32::from(h.height));
    let row = usize::from(h.row_bytes);
    let pixels_row = usize::from(h.width) * 4;
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE || row < pixels_row {
        return Err(DatError::BadSize);
    }
    let expected = row.checked_mul(usize::from(h.height)).filter(|n| *n <= MAX_PIXEL_BYTES).ok_or(DatError::BadSize)?;
    // The chunks, joined: at most the payload's own size.
    let mut zlib = Vec::new();
    loop {
        let len = c.u16()?;
        if len == 0 {
            break;
        }
        zlib.extend_from_slice(c.take(usize::from(len))?);
    }
    // The inflater stops at the promised size, so a bomb costs no more than the image would.
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&zlib, expected).map_err(|e| match e.status {
        miniz_oxide::inflate::TINFLStatus::HasMoreOutput => DatError::Damaged("the pixels run past the image"),
        _ => DatError::Damaged("the pixel data doesn't inflate"),
    })?;
    if raw.len() != expected {
        return Err(DatError::Damaged("the pixel data is shorter than the image"));
    }
    let mut rgba = Vec::with_capacity(usize::from(h.width) * usize::from(h.height) * 4);
    for line in raw.chunks_exact(row) {
        for &[a, r, g, b] in line.as_chunks::<4>().0.iter().take(usize::from(h.width)) {
            if !h.has_alpha {
                rgba.extend_from_slice(&[r, g, b, 255]);
            } else {
                rgba.extend_from_slice(&[straight(r, a), straight(g, a), straight(b, a), a]);
            }
        }
    }
    Ok(DatBitmap { width, height, frame: h.frame, has_alpha: h.has_alpha, rgba })
}

/// A colour channel premultiplied by `a`, made straight (rounded; clamped, since damaged data
/// may hold a channel above its alpha). Fully transparent pixels become black.
fn straight(c: u8, a: u8) -> u8 {
    match a {
        0 => 0,
        255 => c,
        a => ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8,
    }
}

/// How a raw sound payload is laid out: always signed 16-bit little-endian PCM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundFormat {
    /// Samples a second.
    pub rate: u32,
    /// 1 (mono) or 2 (stereo, interleaved left, right).
    pub channels: u16,
}

impl SoundFormat {
    /// From a `DOMSoundItem`'s `format`: `"44kHz 16bit Stereo"`, `"22kHz 16bit Mono"`, …
    pub fn from_xfl(format: &str) -> Result<SoundFormat, DatError> {
        let unknown = || DatError::Unsupported(format!("a sound format of \"{}\"", format.chars().take(64).collect::<String>()));
        let mut words = format.split_whitespace();
        let (Some(rate), Some(bits), Some(channels), None) = (words.next(), words.next(), words.next(), words.next()) else {
            return Err(unknown());
        };
        let rate = match rate.to_ascii_lowercase().strip_suffix("khz").ok_or_else(unknown)? {
            "5" | "5.5" => 5_512,
            "11" => 11_025,
            "22" => 22_050,
            "44" => 44_100,
            _ => return Err(unknown()),
        };
        match bits.to_ascii_lowercase().as_str() {
            "16bit" => {}
            "8bit" => return Err(DatError::Unsupported("8-bit sound data".into())),
            _ => return Err(unknown()),
        }
        let channels = match channels.to_ascii_lowercase().as_str() {
            "mono" => 1,
            "stereo" => 2,
            _ => return Err(unknown()),
        };
        Ok(SoundFormat { rate, channels })
    }

    /// From the format byte of a legacy `.fla`'s `CMediaSound` record, read like SWF's sound
    /// format bits: codec in the high nibble (0, raw), rate index in bits 2–3, 16-bit in bit 1,
    /// stereo in bit 0. `0F` is 44.1 kHz 16-bit stereo and `0E` the same in mono (the only two
    /// observed).
    pub fn from_legacy(byte: u8) -> Result<SoundFormat, DatError> {
        let codec = byte >> 4;
        if codec != 0 {
            return Err(DatError::Unsupported(format!("compressed sound data (codec {codec})")));
        }
        if byte & 0b10 == 0 {
            return Err(DatError::Unsupported("8-bit sound data".into()));
        }
        let rate = match (byte >> 2) & 0b11 {
            0 => 5_512,
            1 => 11_025,
            2 => 22_050,
            _ => 44_100,
        };
        Ok(SoundFormat { rate, channels: if byte & 1 == 1 { 2 } else { 1 } })
    }
}

/// A raw sound payload, checked: whole frames of 16-bit samples, within the limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawSound<'a> {
    pub format: SoundFormat,
    /// Length in frames (samples per channel).
    pub frames: u64,
    /// The payload: signed 16-bit little-endian samples, interleaved.
    pub pcm: &'a [u8],
}

/// Checks a raw sound payload against its item's format and, when the item gives one, its
/// `sampleCount` (frames).
pub fn decode_sound(data: &[u8], format: SoundFormat, sample_count: Option<u64>) -> Result<RawSound<'_>, DatError> {
    if !(1..=2).contains(&format.channels) {
        return Err(DatError::Unsupported(format!("{} sound channels", format.channels)));
    }
    let frame_bytes = u64::from(format.channels) * 2;
    let len = data.len() as u64;
    if let Some(n) = sample_count {
        let expected = n.saturating_mul(frame_bytes);
        if expected != len {
            return Err(DatError::LengthMismatch { expected, found: len });
        }
    }
    if len == 0 {
        return Err(DatError::Empty);
    }
    if !len.is_multiple_of(frame_bytes) {
        return Err(DatError::Damaged("the sound doesn't hold whole sample frames"));
    }
    let frames = len / frame_bytes;
    if frames.saturating_mul(u64::from(format.channels)) > MAX_SOUND_SAMPLES {
        return Err(DatError::TooLong);
    }
    Ok(RawSound { format, frames, pcm: data })
}

impl RawSound<'_> {
    /// The samples, interleaved.
    pub fn samples(&self) -> impl Iterator<Item = i16> + '_ {
        self.pcm.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes(*s))
    }

    /// The sound as a 16-bit PCM WAV file (the payload is already WAV's sample layout, so this
    /// is lossless). `None` only past WAV's 4 GB limit, which [`MAX_SOUND_SAMPLES`] rules out.
    pub fn to_wav(&self) -> Option<Vec<u8>> {
        let mut wav = craft_audio::wav_stream::header(self.format.rate, self.format.channels, craft_audio::wav_stream::Bits::Int16, self.frames)?;
        wav.extend_from_slice(self.pcm);
        Some(wav)
    }
}

#[cfg(test)]
mod tests;
