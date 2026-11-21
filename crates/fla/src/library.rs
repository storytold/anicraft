//! The library as `Contents` lists it beyond its symbols: folders, bitmaps and sounds, and the
//! document's script language (ADR 0096).
//!
//! Learned differentially from the corpus's legacy files and their Animate twins
//! (`DOMFolderItem`, `DOMBitmapItem`, `DOMSoundItem`, `PublishSettings.xml`); no Flash or
//! Animate code involved. Every library item carries the same identity block — u32 6, u32 1,
//! the parent folder's item id, u32 1, its own item id — where an item id is two u32 (a time
//! stamp and a counter, the twin's `itemID` `"%08x-%08x"`) and `0, 0` is the library's root:
//!
//! ```text
//! folder  u32 4 · name (string) · identity · u32 0 · u32 7 · …
//! symbol  01 80 17 · stream name · label · u32 item id · u8 type · string · u32 1 · identity · …
//! media   class tag (CMediaBits, CMediaSound) · 06 · stream name (u8 n, n UTF-16 units) · name ·
//!         u16 media id · source path · u32 import time · identity · u32 0 · u32 7 · u8 ·
//!         3 strings · u8 · u32 2 · 2 strings · 8 bytes · u32 1 · u32 0 · u32 −1 · u8 · string ·
//!         u8 · u32 1 · then a bitmap's 03 … or a sound's 0a · u8 format · u8 · u32 sample count
//! ```
//!
//! A folder's path is its parents' names joined with `/`. The media id is what bitmap
//! instances and frame sounds refer to; it is not the stream's number. Strings are MFC
//! `CArchive` strings ([`crate::text::cstring`]). Every read is bounded: what doesn't parse is
//! left out (and counted by the importer), never guessed at.

use std::collections::HashMap;

/// A library item's id: (time stamp, counter); `(0, 0)` is the library's root.
pub type ItemId = (u32, u32);

/// The root's id.
pub const ROOT: ItemId = (0, 0);
/// Most folders and media items read.
const MAX_ITEMS: usize = 1 << 14;
/// Longest stream name (UTF-16 units).
const MAX_STREAM_NAME: usize = 64;

/// A library folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub parent: ItemId,
    pub id: ItemId,
}

/// A media item's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Bitmap,
    Sound,
}

/// A bitmap or sound library item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaItem {
    /// By its class (declared by name) or its own tail; `None` when neither says.
    pub kind: Option<MediaKind>,
    /// The stream holding its data (`Media 4`, `M 10 1522496704`).
    pub stream: String,
    /// Its name in the library (the leaf: `logo CC88x31.png`, `blink1`).
    pub name: String,
    /// The id placements refer to it by.
    pub id: u16,
    pub parent: ItemId,
    /// A sound's format byte and sample count (frames), when its tail decoded.
    pub sound: Option<(u8, u32)>,
}

/// Bounded little-endian reads.
struct Reader<'a> {
    d: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.d.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn expect(&mut self, v: u32) -> Option<()> {
        (self.u32()? == v).then_some(())
    }
    fn string(&mut self) -> Option<String> {
        let (s, end) = crate::text::cstring(self.d, self.at).ok()?;
        self.at = end;
        Some(s)
    }
    /// The identity block: u32 6, u32 1, parent id, u32 1, own id.
    fn identity(&mut self) -> Option<(ItemId, ItemId)> {
        self.expect(6)?;
        self.expect(1)?;
        let parent = (self.u32()?, self.u32()?);
        self.expect(1)?;
        let id = (self.u32()?, self.u32()?);
        Some((parent, id))
    }
    /// A stream name: a bare u8 length, then UTF-16 units.
    fn stream_name(&mut self) -> Option<String> {
        let n = usize::from(self.u8()?);
        if n == 0 || n > MAX_STREAM_NAME {
            return None;
        }
        let units: Vec<u16> = self.take(n * 2)?.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let s = String::from_utf16(&units).ok()?;
        s.chars().all(|c| c.is_ascii_graphic() || c == ' ').then_some(s)
    }
}

/// The library's folders, in the order `Contents` lists them: each `u32 4`, its name, then the
/// identity block and the fixed `u32 0, u32 7` that starts every item's settings.
pub fn folders(contents: &[u8]) -> Vec<Folder> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= contents.len() && out.len() < MAX_ITEMS {
        if contents.get(i..i + 7) == Some(&[0x04, 0x00, 0x00, 0x00, 0xFF, 0xFE, 0xFF][..]) {
            let mut r = Reader { d: contents, at: i + 4 };
            let folder = (|| {
                let name = r.string()?;
                let (parent, id) = r.identity()?;
                r.expect(0)?;
                r.expect(7)?;
                let name = name.trim().to_string();
                (!name.is_empty() && id != ROOT && !name.contains('/')).then_some(Folder { name, parent, id })
            })();
            if let Some(f) = folder {
                out.push(f);
                i = r.at;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Each folder's path (its parents' names and its own, joined with `/`), by id. A folder whose
/// parent chain is broken or loops sits at the root.
pub fn folder_paths(folders: &[Folder]) -> HashMap<ItemId, String> {
    let by_id: HashMap<ItemId, &Folder> = folders.iter().map(|f| (f.id, f)).collect();
    let mut out = HashMap::new();
    for f in folders {
        let mut parts = vec![f.name.as_str()];
        let mut at = f.parent;
        let mut seen = vec![f.id];
        while at != ROOT && parts.len() < anicraft_doc::library::MAX_FOLDER_DEPTH {
            let Some(p) = by_id.get(&at).filter(|_| !seen.contains(&at)) else { break };
            seen.push(at);
            parts.push(p.name.as_str());
            at = p.parent;
        }
        parts.reverse();
        out.insert(f.id, parts.join("/"));
    }
    out
}

/// A library symbol's parent folder: its entry continues, after the label, item id and type
/// byte at `after` (the type byte's offset + 1), with a string, `u32 1` and the identity block.
pub fn symbol_parent(contents: &[u8], after: usize) -> Option<ItemId> {
    let mut r = Reader { d: contents, at: after };
    r.string()?;
    r.expect(1)?;
    r.identity().map(|(parent, _)| parent)
}

/// The bitmap and sound items, each found at its class tag (a `CMediaBits` / `CMediaSound`
/// declaration or a class reference) followed by version 6 and the name of a stream the file
/// has (`has_stream`).
pub fn media_items(contents: &[u8], has_stream: impl Fn(&str) -> bool) -> Vec<MediaItem> {
    const BITS: &[u8] = b"CMediaBits";
    const SOUND: &[u8] = b"CMediaSound";
    let mut out = Vec::new();
    let mut i = 2usize;
    while i < contents.len() && out.len() < MAX_ITEMS {
        if contents.get(i) != Some(&0x06) {
            i += 1;
            continue;
        }
        let before = |n: usize| i.checked_sub(n).and_then(|s| contents.get(s..i));
        let declared = if before(SOUND.len()) == Some(SOUND) {
            Some(MediaKind::Sound)
        } else if before(BITS.len()) == Some(BITS) {
            Some(MediaKind::Bitmap)
        } else {
            None
        };
        let referenced = contents.get(i - 1) == Some(&0x80);
        if declared.is_none() && !referenced {
            i += 1;
            continue;
        }
        match media_at(contents, i + 1, declared) {
            Some((item, end)) if has_stream(&item.stream) => {
                out.push(item);
                i = end.max(i + 1);
            }
            _ => i += 1,
        }
    }
    out
}

/// One media record from just after its version byte.
fn media_at(contents: &[u8], at: usize, declared: Option<MediaKind>) -> Option<(MediaItem, usize)> {
    let mut r = Reader { d: contents, at };
    let stream = r.stream_name()?;
    let name = r.string()?;
    let id = r.u16()?;
    r.string()?; // the source file's path
    r.u32()?; // its import time
    let (parent, _) = r.identity()?;
    let mut item = MediaItem { kind: declared, stream, name: name.chars().take(256).collect(), id, parent, sound: None };
    let identity_end = r.at;
    // The common settings, then the class's own.
    let tail = (|| {
        r.expect(0)?;
        r.expect(7)?;
        r.u8()?;
        for _ in 0..3 {
            r.string()?;
        }
        r.u8()?;
        r.expect(2)?;
        for _ in 0..2 {
            r.string()?;
        }
        r.take(8)?;
        r.expect(1)?;
        r.expect(0)?;
        r.expect(u32::MAX)?;
        r.u8()?;
        r.string()?;
        r.u8()?;
        r.expect(1)?;
        match r.u8()? {
            0x03 => Some((MediaKind::Bitmap, None)),
            0x0A => {
                let format = r.u8()?;
                r.u8()?;
                Some((MediaKind::Sound, Some((format, r.u32()?))))
            }
            _ => None,
        }
    })();
    match tail {
        Some((kind, sound)) if declared.is_none_or(|d| d == kind) => {
            item.kind = Some(kind);
            item.sound = sound;
            Some((item, r.at))
        }
        _ => Some((item, identity_end)),
    }
}

/// The document's `ActionScriptVersion` publish setting (`"2"`), from the `Vector::` key and
/// value strings `Contents` holds.
pub fn script_version(contents: &[u8]) -> Option<String> {
    let key: Vec<u8> = "Vector::ActionScriptVersion".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut needle = vec![0xFF, 0xFE, 0xFF, 27];
    needle.extend_from_slice(&key);
    let at = contents.windows(needle.len()).position(|w| w == needle.as_slice())?;
    let (value, _) = crate::text::cstring(contents, at.checked_add(needle.len())?).ok()?;
    Some(value.trim().chars().take(16).collect())
}
