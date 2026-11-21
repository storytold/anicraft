//! AniCraft legacy Flash import (layer L2): the Flash 8–CS4 era `.fla` container and its
//! record streams.
//!
//! A `.fla` saved by Flash 8 through CS4 is an OLE2 **compound binary file** (Microsoft
//! `[MS-CFB]`), unlike the zip packages Flash CS5 and Animate write (imported by `anicraft-xfl`,
//! ADR 0009 and ADR 0054). This crate reads the container from the public specification —
//! its own reader, like every format in the workspace — and exposes the document's streams
//! (`Contents`, `S <n> <stamp>` library items, `Symbol <n>` pages, `Media <n>` bitmaps and
//! sounds). A tag scanner [`records`] lays out each stream's `CPic*`/`C*` record names, the
//! reverse-engineering microscope behind `anicraft-cli fla-dump`.
//!
//! Container hardening follows ADR 0054's zip profile: the file is capped at 1 GiB and every
//! stream at 256 MiB, chains (FAT, mini-FAT, DIFAT, directory tree) are cycle-guarded, nothing
//! is written to disk, and hostile or damaged input is refused with a one-sentence error —
//! never a panic. [`archive`] reads an item stream's object graph (layers, keyframes, symbol
//! instances, tweens), [`shape`] the shape records' styles and straight and curved edges,
//! [`text`] the `CPicText` records' static and dynamic text fields, [`library`] the folders,
//! bitmaps and sounds `Contents` lists, and [`doc`] maps them into the AniCraft model the way the
//! XFL importer maps Animate's (ADR 0096).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod archive;
pub mod cfb;
pub mod doc;
pub mod library;
pub mod records;
pub mod report;
pub mod shape;
pub mod text;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_archive;
#[cfg(test)]
mod tests_bounds;
#[cfg(test)]
mod text_tests;

pub use doc::import;
pub use report::{ImportError, Report};

/// The OLE2 compound-document magic: the first bytes of a Flash 8–CS4 `.fla`.
pub const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The file's first bytes are the legacy compound-binary magic (its name doesn't matter).
pub fn is_legacy(data: &[u8]) -> bool {
    data.len() >= 8 && data[..8] == MAGIC
}

/// Everything the `fla-dump` microscope reports about one stream.
pub struct StreamDump {
    pub path: String,
    pub size: u64,
    /// The stream's first bytes, as `aa bb …`.
    pub head: String,
    /// The record tags the scanner found, in order.
    pub tags: Vec<(usize, String)>,
    /// False when the scan stopped at [`records::MAX_TAGS`], which all of a file's streams
    /// share: `tags` are the first ones only.
    pub tags_complete: bool,
}

/// Dumps a legacy document's streams for triage and reverse-engineering.
pub fn dump(data: &[u8]) -> Result<Vec<StreamDump>, cfb::Error> {
    let file = cfb::Compound::new(data)?;
    let mut out = Vec::new();
    // One tag budget for the whole file: several directory entries may share one chain, so a
    // budget per stream would let a small file hold millions of tags.
    let mut tags_left = records::MAX_TAGS;
    for entry in file.entries() {
        if entry.kind != cfb::Kind::Stream {
            continue;
        }
        let bytes = file.stream(&entry.path)?;
        let scan = records::scan_tags_max(&bytes, tags_left);
        tags_left = tags_left.saturating_sub(scan.tags.len());
        out.push(StreamDump {
            path: entry.path.clone(),
            size: entry.size,
            head: bytes.iter().take(8).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
            tags: scan.tags.into_iter().map(|t| (t.offset, t.name)).collect(),
            tags_complete: scan.complete,
        });
    }
    Ok(out)
}
