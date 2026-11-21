//! Record-tag scanning: the vocabulary a legacy stream's `CPic*`/`C*` records lay out.
//!
//! Observed on real Flash 8–CS4 files (see the local corpus notes): after a 5-byte stream
//! prologue (typically `01 FF FF 01 00`), a stream holds records whose name is written as
//! a 16-bit little-endian length followed by that many ASCII bytes (`08 00 "CPicPage"`,
//! `09 00 "CPicLayer"`, `09 00 "CPicFrame"`, `09 00 "CPicShape"`, …). Scanning only needs
//! to recognise such name fields — finding them at every offset where a plausible length
//! is followed by printable ASCII — which recovers the stream's record vocabulary and
//! structure without guessing at the rest of the binary payload. This is deliberately
//! conservative: tags shorter than four characters are noise from binary data, not names.

/// One record name and where it sits in the stream's bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    /// Offset of the name's length prefix.
    pub offset: usize,
    /// The record's name, e.g. `"CPicShape"`.
    pub name: String,
}

/// The most tags one scan keeps: far more than any corpus stream holds (ten at most), so a
/// stream of nothing but name-shaped bytes can't make the microscope hold millions.
pub const MAX_TAGS: usize = 1 << 16;

/// A scan's record names, and whether it saw the whole stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub tags: Vec<Tag>,
    /// False when the scan stopped at its budget: `tags` are the stream's first ones only.
    pub complete: bool,
}

/// Scans a stream's bytes for record names, in order, keeping at most [`MAX_TAGS`].
pub fn scan_tags(data: &[u8]) -> Scan {
    scan_tags_max(data, MAX_TAGS)
}

/// Scans a stream's bytes for record names, in order, keeping at most `max`.
pub fn scan_tags_max(data: &[u8], max: usize) -> Scan {
    let mut tags = Vec::new();
    let mut i = 0usize;
    while i + 2 <= data.len() {
        let len = u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        if (4..=64).contains(&len) && i + 2 + len <= data.len() && data[i + 2..i + 2 + len].iter().all(|b| (0x20..0x7f).contains(b)) {
            let name = String::from_utf8_lossy(&data[i + 2..i + 2 + len]).into_owned();
            // A name field starts a record only if it is the stream's usual `C`-prefixed
            // class shape; binary data happens to match occasionally, so require it.
            if name.starts_with('C') && name.chars().nth(1).is_some_and(|c| c.is_ascii_uppercase()) {
                if tags.len() >= max {
                    return Scan { tags, complete: false };
                }
                tags.push(Tag { offset: i, name });
                i += 2 + len;
                continue;
            }
        }
        i += 1;
    }
    Scan { tags, complete: true }
}

/// The 5-byte prologue the library-item streams begin with (`01 FF FF 01 00`).
pub const ITEM_PROLOGUE: [u8; 5] = [0x01, 0xFF, 0xFF, 0x01, 0x00];

/// The 5-byte prologue the `Contents` document stream begins with.
pub const CONTENTS_PROLOGUE: [u8; 5] = [0x3F, 0x01, 0x00, 0x00, 0x00];

/// The 5-byte prologue the `Media` streams begin with.
pub const MEDIA_PROLOGUE: [u8; 5] = [0x03, 0x05, 0x60, 0x01, 0x58];
