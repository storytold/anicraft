//! The compound binary container, from Microsoft [MS-CFB] (the OLE2 compound file format).
//!
//! The reader answers stream lookups from the file's own bytes: nothing is extracted to
//! disk, and only stream contents are copied. Structures the specification marks as fixed
//! (sector shifts, the mini-stream cutoff) are validated, not trusted; every chain walk is
//! cycle-guarded and bounded, so a damaged or hostile file is refused, never crash-prone.

use std::fmt;

/// Hard caps, mirroring the hardened zip reader's profile (ADR 0054): 1 GiB per file and
/// 256 MiB per stream, and a bound on every chain walk.
pub const MAX_FILE: u64 = 1 << 30;
const MAX_STREAM: u64 = 256 << 20;
const MAX_CHAIN: usize = 1 << 22;
const MAX_TREE_DEPTH: usize = 64;
const MAX_ENTRIES: usize = 1 << 20;

/// The mini-stream cutoff the specification fixes for every compound file.
const CUTOFF: u64 = 0x1000;

/// Why a container couldn't be read: one sentence each, safe to show.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("not a legacy Flash document: its first bytes are not the compound binary signature")]
    NotCompound,
    #[error("this legacy Flash document is larger than AniCraft reads (over 1 GiB)")]
    TooLarge,
    #[error("this legacy Flash document is damaged: the file ends inside a structure the container claims")]
    Truncated,
    #[error("this legacy Flash document is damaged: {0}")]
    BadHeader(&'static str),
    #[error("this legacy Flash document is damaged: the container's allocation chains loop")]
    ChainCycle,
    #[error("this legacy Flash document is damaged: an allocation chain runs past the structure it holds")]
    ChainTooLong,
    #[error("this legacy Flash document is damaged: its directory tree loops or is too deep")]
    TreeCycle,
    #[error("this legacy Flash document claims a stream far larger than the file could hold")]
    StreamTooLarge,
    #[error("this legacy Flash document is damaged: a stream name is malformed")]
    NameInvalid,
    #[error("no stream named {0:?} in this legacy Flash document")]
    NotFound(String),
}

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A stream: its bytes can be read.
    Stream,
    /// A storage: it holds other entries.
    Storage,
}

/// One entry of the container's directory, with its full path (`"S 49 1724827326"`).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    /// The stream's size in bytes (0 for storages).
    pub size: u64,
    /// The stream's first sector (its chain's start; debug/forensics, not needed to read).
    pub start: u32,
}

/// FAT chain markers the specification reserves.
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
const NOSTREAM: u32 = 0xFFFF_FFFF;

/// An open compound file over the caller's bytes.
pub struct Compound<'a> {
    data: &'a [u8],
    /// Sector size in bytes (512 for major version 3, 4096 for 4).
    sector: usize,
    /// FAT: sector chains, indexed by sector id.
    fat: Vec<u32>,
    /// Mini-FAT: 64-byte mini-sector chains inside the root's stream, indexed by mini-sector id.
    minifat: Vec<u32>,
    /// The root storage's stream, which holds the mini stream.
    mini_stream: Vec<u8>,
    /// Directory entries with paths, root first, then the tree in walk order.
    entries: Vec<Entry>,
}

impl<'a> Compound<'a> {
    /// Opens a compound file over `data`, validating its header, FAT and directory.
    pub fn new(data: &'a [u8]) -> Result<Compound<'a>, Error> {
        if !crate::is_legacy(data) {
            return Err(Error::NotCompound);
        }
        if data.len() as u64 > MAX_FILE {
            return Err(Error::TooLarge);
        }
        let h = data.get(..512).ok_or(Error::Truncated)?;
        let u16_at = |o: usize| -> u16 { u16::from_le_bytes([h[o], h[o + 1]]) };
        let u32_at = |o: usize| -> u32 { u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]) };

        let major = u16_at(26);
        let sector_shift = u16_at(30);
        if major != 3 && major != 4 {
            return Err(Error::BadHeader("the container's version is neither 3 nor 4"));
        }
        let expected_shift = if major == 3 { 9 } else { 12 };
        if sector_shift != expected_shift {
            return Err(Error::BadHeader("the container's sector size doesn't match its version"));
        }
        // Shifted only once validated: the file's own shift could overflow.
        let sector = 1usize << expected_shift;
        if mini_shift(h) != 6 {
            return Err(Error::BadHeader("the mini-sector size isn't 64 bytes"));
        }
        if u32_at(56) as u64 != CUTOFF {
            return Err(Error::BadHeader("the mini-stream cutoff isn't 4096 bytes"));
        }
        if u16_at(28) != 0xFFFE {
            return Err(Error::BadHeader("the container's byte order isn't little-endian"));
        }
        let sectors_in_file = data.len() / sector;

        // The FAT sector list: 109 in the header, then chained extended DIFAT sectors. Each
        // is a distinct sector of the file, so the claimed count is bounded by the file
        // before anything is reserved for it.
        let fat_count = u32_at(44) as usize;
        let mut listed = vec![false; sectors_in_file.saturating_sub(1)];
        if fat_count > listed.len() {
            return Err(Error::Truncated);
        }
        let difat_first = u32_at(68);
        let difat_count = u32_at(72) as usize;
        let mut fat_sectors: Vec<u32> = Vec::new();
        for i in 0..109 {
            list_fat_sector(u32_at(76 + i * 4), &mut listed, &mut fat_sectors, fat_count)?;
        }
        if difat_first != ENDOFCHAIN && difat_count == 0 {
            return Err(Error::BadHeader("the container points at DIFAT sectors it says it doesn't have"));
        }
        let difat_per_sector = sector / 4 - 1;
        let mut next = difat_first;
        let mut difat_visited = std::collections::HashSet::new();
        let mut difat_seen = 0usize;
        while next != ENDOFCHAIN {
            if difat_seen >= difat_count || !difat_visited.insert(next) {
                return Err(Error::ChainCycle);
            }
            difat_seen += 1;
            let s = Self::sector_of(data, next, sector).ok_or(Error::Truncated)?;
            for i in 0..difat_per_sector {
                let id = u32::from_le_bytes([s[i * 4], s[i * 4 + 1], s[i * 4 + 2], s[i * 4 + 3]]);
                list_fat_sector(id, &mut listed, &mut fat_sectors, fat_count)?;
            }
            next = u32::from_le_bytes([s[difat_per_sector * 4], s[difat_per_sector * 4 + 1], s[difat_per_sector * 4 + 2], s[difat_per_sector * 4 + 3]]);
        }
        if fat_sectors.len() != fat_count {
            return Err(Error::BadHeader("the container's FAT sector count doesn't match its DIFAT"));
        }

        let mut fat = Vec::with_capacity(fat_sectors.len() * sector / 4);
        for &id in &fat_sectors {
            let s = Self::sector_of(data, id, sector).ok_or(Error::Truncated)?;
            for w in 0..sector / 4 {
                fat.push(u32::from_le_bytes([s[w * 4], s[w * 4 + 1], s[w * 4 + 2], s[w * 4 + 3]]));
            }
        }

        // The directory: sectors chained through the FAT, 128-byte entries in each, read no
        // further than the most entries a directory may hold.
        let mut dir_bytes = Vec::new();
        read_chain(data, &fat, u32_at(48), sector, &mut dir_bytes, sectors_in_file.min(dir_sectors_max(sector)))?;
        let entry_count = dir_bytes.len() / 128;
        if entry_count == 0 || entry_count > MAX_ENTRIES {
            return Err(Error::BadHeader("the container's directory is empty or too large"));
        }

        // Entry fields, validated: name, type, size, start sector.
        let record = |id: usize| -> Result<&[u8], Error> { dir_bytes.get(id * 128..id * 128 + 128).ok_or(Error::Truncated) };
        let name_of = |id: usize| -> Result<String, Error> {
            let e = record(id)?;
            let len = u16::from_le_bytes([e[64], e[65]]) as usize;
            if !(2..=64).contains(&len) || !len.is_multiple_of(2) {
                return Err(Error::NameInvalid);
            }
            let text: Vec<u16> = e[..len].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            // The name ends before its nul terminator, and has no nul inside.
            let chars: &[u16] = match text.iter().position(|&c| c == 0) {
                Some(n) if n > 0 && n == text.len() - 1 => &text[..n],
                Some(_) => return Err(Error::NameInvalid),
                None => &text,
            };
            String::from_utf16(chars).map_err(|_| Error::NameInvalid)
        };
        let kind_of = |id: usize| -> Result<Option<Kind>, Error> {
            // Type 5 is the root storage; 1 a storage; 2 a stream. Types 0 (unallocated),
            // 3 (lock bytes) and 4 (property set) never hold document streams: they are
            // left out rather than refusing the file.
            match record(id)?[66] {
                1 | 5 => Ok(Some(Kind::Storage)),
                2 => Ok(Some(Kind::Stream)),
                0 | 3 | 4 => Ok(None),
                _ => Err(Error::BadHeader("a directory entry has an unknown object type")),
            }
        };
        let u32_field = |id: usize, at: usize| -> Result<u32, Error> {
            let e = record(id)?;
            Ok(u32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]))
        };
        let size_of = |id: usize| -> Result<u64, Error> {
            let e = record(id)?;
            let size = u64::from_le_bytes([e[120], e[121], e[122], e[123], e[124], e[125], e[126], e[127]]);
            // Version 3 uses only the low 32 bits of a stream's size.
            Ok(if major == 3 { size & 0xFFFF_FFFF } else { size })
        };

        // Walk the directory tree from the root's child, so entries carry full paths.
        // Each stack item is (entry id, parent path, depth); left/right siblings share
        // their parent's path, a child carries its own.
        let mut entries = vec![Entry { path: String::new(), kind: Kind::Storage, size: size_of(0)?, start: u32_field(0, 116)? }];
        let mut visited = vec![false; entry_count];
        visited[0] = true;
        let mut stack: Vec<(usize, String, usize)> = Vec::new();
        let root_child = u32_field(0, 76)?;
        if root_child != NOSTREAM {
            stack.push((root_child as usize, String::new(), 1));
        }
        while let Some((id, parent_path, depth)) = stack.pop() {
            if id >= entry_count || visited[id] || depth > MAX_TREE_DEPTH {
                return Err(Error::TreeCycle);
            }
            visited[id] = true;
            let kind = match kind_of(id)? {
                Some(k) => k,
                None => continue, // not a named entry: never part of the tree's names
            };
            let name = name_of(id)?;
            let path = if parent_path.is_empty() { name } else { format!("{parent_path}/{name}") };
            let size = if kind == Kind::Stream { size_of(id)? } else { 0 };
            if size > MAX_STREAM {
                return Err(Error::StreamTooLarge);
            }
            let start = u32_field(id, 116)?;
            for nibling in [u32_field(id, 68)?, u32_field(id, 72)?] {
                if nibling != NOSTREAM {
                    stack.push((nibling as usize, parent_path.clone(), depth + 1));
                }
            }
            if kind == Kind::Storage {
                let child = u32_field(id, 76)?;
                if child != NOSTREAM {
                    stack.push((child as usize, path.clone(), depth + 1));
                }
            }
            entries.push(Entry { path, kind, size, start });
        }

        // The mini stream lives in the root storage's stream, in 64-byte mini sectors.
        let mini_stream = if entries[0].size > 0 {
            let size = usize::try_from(entries[0].size).map_err(|_| Error::StreamTooLarge)?;
            if size > data.len() {
                return Err(Error::Truncated);
            }
            copy_chain(entries[0].start, size, sector, |id| fat.get(id as usize).copied(), |id| Self::sector_of(data, id, sector))?
        } else {
            Vec::new()
        };

        let mut minifat = Vec::new();
        let mut mfb = Vec::new();
        read_chain(data, &fat, u32_at(60), sector, &mut mfb, sectors_in_file)?;
        for w in 0..mfb.len() / 4 {
            minifat.push(u32::from_le_bytes([mfb[w * 4], mfb[w * 4 + 1], mfb[w * 4 + 2], mfb[w * 4 + 3]]));
        }

        Ok(Compound { data, sector, fat, minifat, mini_stream, entries })
    }

    fn sector_of(data: &[u8], id: u32, sector: usize) -> Option<&[u8]> {
        // Sector ids count from the end of the 512-byte header: id N is the Nth
        // 512-byte block after it.
        let start = ((id as usize).checked_add(1)?).checked_mul(sector)?;
        data.get(start..start.checked_add(sector)?)
    }

    /// Every entry, root first, then the tree in walk order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Reads a stream by its full path. Storages have nothing to read.
    pub fn stream(&self, path: &str) -> Result<Vec<u8>, Error> {
        let entry = self.entries.iter().find(|e| e.kind == Kind::Stream && e.path == path).ok_or_else(|| Error::NotFound(path.to_string()))?;
        if entry.size > MAX_STREAM || entry.size > self.data.len() as u64 {
            return Err(Error::StreamTooLarge);
        }
        let size = entry.size as usize;
        if entry.size < CUTOFF {
            // A mini stream: 64-byte mini sectors chained by the mini-FAT, inside the
            // root storage's stream.
            copy_chain(entry.start, size, 64, |id| self.minifat.get(id as usize).copied(), |id| {
                let start = (id as usize).checked_mul(64)?;
                self.mini_stream.get(start..start.checked_add(64)?)
            })
        } else {
            copy_chain(entry.start, size, self.sector, |id| self.fat.get(id as usize).copied(), |id| Self::sector_of(self.data, id, self.sector))
        }
    }
}

fn mini_shift(h: &[u8]) -> u16 {
    u16::from_le_bytes([h[32], h[33]])
}

/// Adds one FAT sector id from the DIFAT (FREESECT slots are unused), refusing an id past
/// the file's sectors, a sector listed twice, and more sectors than the header claims.
fn list_fat_sector(id: u32, listed: &mut [bool], fat_sectors: &mut Vec<u32>, fat_count: usize) -> Result<(), Error> {
    if id == FREESECT {
        return Ok(());
    }
    let seen = listed.get_mut(id as usize).ok_or(Error::Truncated)?;
    if *seen {
        return Err(Error::BadHeader("the container lists a FAT sector twice"));
    }
    *seen = true;
    if fat_sectors.len() >= fat_count {
        return Err(Error::BadHeader("the container lists more FAT sectors than it claims"));
    }
    fat_sectors.push(id);
    Ok(())
}

/// The most sectors a directory of [`MAX_ENTRIES`] 128-byte entries spans.
pub(crate) fn dir_sectors_max(sector: usize) -> usize {
    (MAX_ENTRIES * 128).div_ceil(sector)
}

/// Copies a stream of `size` bytes from its chain of `unit`-byte sectors: exactly the
/// sectors the size needs, and of the last only the bytes it declares. A chain that ends
/// before them is [`Error::Truncated`]. Links past them are never followed: the declared
/// size bounds the work, and slack sectors at a chain's end cost nothing, so a writer that
/// leaves some still opens. An empty stream reads nothing, wherever its chain claims to start.
pub(crate) fn copy_chain<'d>(first: u32, size: usize, unit: usize, next_of: impl Fn(u32) -> Option<u32>, sector_of: impl Fn(u32) -> Option<&'d [u8]>) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(size);
    let mut next = first;
    let mut visited = std::collections::HashSet::new();
    while out.len() < size {
        if next == ENDOFCHAIN || next == FREESECT {
            return Err(Error::Truncated);
        }
        if !visited.insert(next) || visited.len() > MAX_CHAIN {
            return Err(Error::ChainCycle);
        }
        let bytes = sector_of(next).ok_or(Error::Truncated)?;
        let take = (size - out.len()).min(unit);
        out.extend_from_slice(bytes.get(..take).ok_or(Error::Truncated)?);
        if out.len() < size {
            next = next_of(next).ok_or(Error::Truncated)?;
        }
    }
    Ok(out)
}

/// Reads a FAT chain of whole sectors into `out`, cycle-guarded and bounded. An empty
/// chain starts at ENDOFCHAIN (or FREESECT, for a stream that holds nothing).
pub(crate) fn read_chain(data: &[u8], fat: &[u32], first: u32, sector: usize, out: &mut Vec<u8>, max_sectors: usize) -> Result<(), Error> {
    let mut next = first;
    let mut visited = std::collections::HashSet::new();
    while next != ENDOFCHAIN && next != FREESECT {
        if !visited.insert(next) || visited.len() > MAX_CHAIN {
            return Err(Error::ChainCycle);
        }
        if out.len() / sector >= max_sectors {
            return Err(Error::ChainTooLong);
        }
        let start = ((next as usize).checked_add(1).ok_or(Error::Truncated)?).checked_mul(sector).ok_or(Error::Truncated)?;
        let bytes = data.get(start..start.checked_add(sector).ok_or(Error::Truncated)?).ok_or(Error::Truncated)?;
        out.extend_from_slice(bytes);
        next = *fat.get(next as usize).ok_or(Error::Truncated)?;
    }
    Ok(())
}

impl fmt::Debug for Compound<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Compound").field("sector", &self.sector).field("entries", &self.entries.len()).finish()
    }
}
