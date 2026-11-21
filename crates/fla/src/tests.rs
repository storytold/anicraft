//! Container tests, with a compound-file writer built independently from the reader —
//! from Microsoft [MS-CFB] alone, like the hardened zip reader's corpus writer (ADR 0054).

use super::cfb::{Compound, Error, Kind, MAX_FILE};
use super::records;

/// Builds a version-3 compound file holding named streams. Streams shorter than the
/// 4096-byte cutoff go through the mini stream, longer ones through the FAT — so both
/// paths are exercised by construction. (`name`, `bytes`) pairs; names are ASCII.
pub(crate) fn write(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    const SECTOR: usize = 512;
    const CUTOFF: usize = 0x1000;
    const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
    const FREESECT: u32 = 0xFFFF_FFFF;

    // Layout: header, then big streams, then the mini stream, the directory, the mini
    // FAT, and the FAT. The header comes back and is filled in last.
    let mut out = vec![0u8; 512];
    let mut pos = 512usize;

    let (mini, big): (Vec<_>, Vec<_>) = streams.iter().map(|(n, b)| (n.to_string(), b.clone())).partition(|(_, b)| b.len() < CUTOFF);

    // Big streams: whole sectors each.
    let mut big_starts: Vec<(String, u32, u64)> = Vec::new();
    for (name, bytes) in &big {
        let first = (pos / SECTOR - 1) as u32;
        big_starts.push((name.clone(), first, bytes.len() as u64));
        let mut padded = bytes.clone();
        if let Some(rem) = padded.len().checked_rem(SECTOR) && rem > 0 {
            padded.resize(padded.len() + (SECTOR - rem), 0);
        }
        out.extend_from_slice(&padded);
        pos += padded.len();
    }

    // The mini stream: every small stream's bytes, packed into 64-byte mini sectors,
    // then padded to whole sectors.
    let mut mini_stream = Vec::new();
    let mut mini_starts: Vec<(String, u32, u64)> = Vec::new();
    for (name, bytes) in &mini {
        let first = (mini_stream.len() / 64) as u32;
        mini_starts.push((name.clone(), first, bytes.len() as u64));
        mini_stream.extend_from_slice(bytes);
        if let Some(rem) = bytes.len().checked_rem(64) && rem > 0 {
            mini_stream.resize(mini_stream.len() + (64 - rem), 0);
        }
    }
    let mut mini_stream_sectors: Vec<u32> = Vec::new();
    if !mini_stream.is_empty() {
        let first = (pos / SECTOR - 1) as u32;
        let rem = mini_stream.len() % SECTOR;
        if rem > 0 {
            mini_stream.resize(mini_stream.len() + (SECTOR - rem), 0);
        }
        out.extend_from_slice(&mini_stream);
        for s in 0..mini_stream.len() / SECTOR {
            mini_stream_sectors.push(first + s as u32);
        }
        pos += mini_stream.len();
    }

    // The directory: a root entry plus one per stream; the tree is a flat sibling chain
    // from the root's child, each entry's right sibling the next.
    let mut entries: Vec<Vec<u8>> = Vec::new();
    let name_bytes = |name: &str| -> Vec<u8> { name.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let mut root = vec![0u8; 128];
    let root_name = name_bytes("Root Entry");
    root[..root_name.len()].copy_from_slice(&root_name);
    root[64] = (root_name.len() + 2) as u8;
    root[66] = 5; // the root storage
    for at in [68, 72] {
        root[at..at + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    }
    root[120..128].copy_from_slice(&(mini_stream.len() as u64).to_le_bytes());
    entries.push(root);    for (name, _) in streams {
        let mut e = vec![0u8; 128];
        let bytes = name_bytes(name);
        e[..bytes.len()].copy_from_slice(&bytes);
        e[64] = (bytes.len() + 2) as u8;
        e[66] = 2; // a stream
        // Unused tree links are NOSTREAM, not zero: 0 is a valid directory id.
        for at in [68, 72, 76] {
            e[at..at + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        }
        entries.push(e);
    }
    for (name, first, size) in big_starts.iter().chain(&mini_starts) {
        let e = entries.iter_mut().find(|e| e.starts_with(&name_bytes(name))).expect("written entry");
        e[116..120].copy_from_slice(&first.to_le_bytes());
        e[120..128].copy_from_slice(&size.to_le_bytes());
    }
    for i in 1..entries.len() {
        if i + 1 < entries.len() {
            entries[i][72..76].copy_from_slice(&(i as u32 + 1).to_le_bytes());
        }
    }
    if entries.len() > 1 {
        entries[0][76..80].copy_from_slice(&1u32.to_le_bytes());
    }
    if let Some(&first) = mini_stream_sectors.first() {
        entries[0][116..120].copy_from_slice(&first.to_le_bytes()); // the mini stream
    } else {
        entries[0][116..120].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    }
    let dir_start = (pos / SECTOR - 1) as u32;
    let dir_sectors = (entries.len() * 128).div_ceil(SECTOR);
    let mut dir_padded = entries.concat();
    dir_padded.resize(dir_sectors * SECTOR, 0xFF);
    out.extend_from_slice(&dir_padded);
    pos += dir_padded.len();

    // The mini FAT: one entry per 64-byte mini sector, chaining each small stream.
    let mini_sectors_total = mini_stream.len() / 64;
    let mut minifat = vec![ENDOFCHAIN; mini_sectors_total.max(1)];
    for (_, first, size) in &mini_starts {
        let n = (*size as usize).div_ceil(64);
        for i in 0..n {
            let id = *first as usize + i;
            minifat[id] = if i + 1 < n { *first + i as u32 + 1 } else { ENDOFCHAIN };
        }
    }
    let minifat_start = (pos / SECTOR - 1) as u32;
    let mut mf_padded = minifat.iter().flat_map(|v| u32::to_le_bytes(*v)).collect::<Vec<u8>>();
    let mf_sectors = mf_padded.len().div_ceil(SECTOR);
    mf_padded.resize(mf_sectors * SECTOR, 0xFF);
    out.extend_from_slice(&mf_padded);
    pos += mf_padded.len();

    // The FAT: the FAT's own chain first, then the big streams, the directory, the mini
    // stream and the mini FAT; everything else free.
    let fat_start = (pos / SECTOR - 1) as u32;
    let fat_entries_needed = pos / SECTOR + 1;
    let fat_sectors = fat_entries_needed.div_ceil(SECTOR / 4);
    let mut fat = vec![FREESECT; fat_sectors * SECTOR / 4];
    for (i, entry) in fat.iter_mut().enumerate().take(fat_sectors) {
        *entry = if i + 1 < fat_sectors { fat_start + i as u32 + 1 } else { ENDOFCHAIN };
    }
    let chain = |fat: &mut Vec<u32>, first: u32, count: usize| {
        for i in 0..count {
            let id = first as usize + i;
            fat[id] = if i + 1 < count { first + i as u32 + 1 } else { ENDOFCHAIN };
        }
    };
    for (_, first, size) in &big_starts {
        chain(&mut fat, *first, (*size as usize).div_ceil(SECTOR));
    }
    chain(&mut fat, dir_start, dir_sectors);
    if !mini_stream_sectors.is_empty() {
        chain(&mut fat, mini_stream_sectors[0], mini_stream_sectors.len());
    }
    chain(&mut fat, minifat_start, mf_sectors);
    out.extend_from_slice(&fat.iter().flat_map(|v| u32::to_le_bytes(*v)).collect::<Vec<u8>>());

    // The header, finally.
    let h = &mut out[..512];
    h[0..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    h[24..26].copy_from_slice(&0x003Eu16.to_le_bytes()); // minor version
    h[26..28].copy_from_slice(&3u16.to_le_bytes()); // major version, 512-byte sectors
    h[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes()); // little-endian
    h[30..32].copy_from_slice(&9u16.to_le_bytes());
    h[32..34].copy_from_slice(&6u16.to_le_bytes()); // 64-byte mini sectors
    h[44..48].copy_from_slice(&(fat_sectors as u32).to_le_bytes());
    h[48..52].copy_from_slice(&dir_start.to_le_bytes());
    h[56..60].copy_from_slice(&0x1000u32.to_le_bytes()); // mini-stream cutoff
    h[60..64].copy_from_slice(&minifat_start.to_le_bytes());
    h[64..68].copy_from_slice(&(mf_sectors as u32).to_le_bytes());
    h[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes()); // no extended DIFAT
    h[72..76].copy_from_slice(&0u32.to_le_bytes());
    for i in 0..109 {
        let id = if i < fat_sectors { fat_start + i as u32 } else { FREESECT };
        h[76 + i * 4..80 + i * 4].copy_from_slice(&id.to_le_bytes());
    }
    out
}

pub(crate) fn entry_record(f: &[u8], name: &str) -> usize {
    let needle = name.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
    let at = f.windows(needle.len()).position(|w| w == needle).expect("entry found");
    at - (at % 128) // the entry's 128-byte record, absolutely addressed
}

fn set_u32(f: &mut [u8], at: usize, v: u32) {
    f[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[test]
fn streams_round_trip_through_the_container() {
    let payloads: Vec<(&str, Vec<u8>)> = vec![
        ("Contents", b"hello document".to_vec()),
        ("S 1 100", (0..5000).map(|i| (i % 251) as u8).collect()),
        ("Media 1", vec![0xAA; 300]),
        ("Symbol 1", b"x".to_vec()),
        ("S 2 100", vec![0x55; 4096]), // exactly the cutoff: the FAT path
    ];
    let bytes = write(&payloads);
    let file = Compound::new(&bytes).expect("our own writer's output opens");
    let names: Vec<&str> = file.entries().iter().filter_map(|e| if e.path.is_empty() { None } else { Some(e.path.as_str()) }).collect();
    assert_eq!(names, vec!["Contents", "S 1 100", "Media 1", "Symbol 1", "S 2 100"]);
    for (name, want) in &payloads {
        let got = file.stream(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(got, *want, "{name}");
    }
    assert_eq!(file.entries()[0].kind, Kind::Storage);
}

#[test]
fn a_file_that_is_not_a_compound_file_is_refused() {
    assert!(matches!(Compound::new(b"PK\x03\x04 whatever"), Err(Error::NotCompound)));
    assert!(matches!(Compound::new(&[]), Err(Error::NotCompound)));
}

#[test]
fn a_truncated_file_is_refused() {
    let good = write(&[("Contents", b"hi".to_vec())]);
    for cut in [100usize, 512, 1024, good.len() - 1] {
        assert!(matches!(Compound::new(&good[..cut]), Err(Error::Truncated | Error::NotCompound)), "cut at {cut}");
    }
}

#[test]
fn a_fat_chain_that_loops_is_refused() {
    let mut f = write(&[("Contents", vec![0; 8192])]); // a multi-sector stream
    // Find the FAT sector by its chain words: the big stream's chain (sector 1 → 2, 3, …)
    // sits after the FAT's own chain words, so look for `2, 3` as the second and third words.
    let fat_at = (0..f.len()).step_by(512)
        .find(|&c| f[c + 4] == 2 && f[c + 5] == 0 && f[c + 6] == 0 && f[c + 7] == 0 && f[c + 8] == 3 && f[c + 12] == 4)
        .expect("the FAT sector found by its chain words");
    // Every chain word that names a real sector becomes a self-loop.
    for w in 0..(f.len() - fat_at) / 4 {
        let at = fat_at + w * 4;
        let v = u32::from_le_bytes(f[at..at + 4].try_into().unwrap());
        if v != ENDOFCHAIN && v != FREESECT && v != FATSECT_MARKER && v > 0 && v < (f.len() / 512) as u32 {
            set_u32(&mut f, at, w as u32);
        }
    }
    // Opening validates the header, directory and mini-FAT chains; the stream's own
    // chain is walked when the stream is read.
    if let Ok(file) = Compound::new(&f) {
        assert!(matches!(file.stream("Contents"), Err(Error::ChainCycle | Error::Truncated)), "the looping stream chain must be refused");
    }
}

const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
const FATSECT_MARKER: u32 = 0xFFFF_FFFC;

#[test]
fn a_header_that_lies_about_its_structure_is_refused() {
    let mut f = write(&[("Contents", b"hi".to_vec())]);
    f[30] = 10; // a sector shift that doesn't match version 3
    assert!(matches!(Compound::new(&f), Err(Error::BadHeader(_) | Error::Truncated)));

    let mut f = write(&[("Contents", b"hi".to_vec())]);
    f[44] = 99; // more FAT sectors than the header lists
    assert!(matches!(Compound::new(&f), Err(Error::BadHeader(_) | Error::Truncated | Error::ChainCycle)));

    let mut f = write(&[("Contents", b"hi".to_vec())]);
    f[28] = 0; // the byte order mark broken
    assert!(matches!(Compound::new(&f), Err(Error::BadHeader(_))));
}

#[test]
fn a_directory_tree_that_loops_is_refused() {
    let mut f = write(&[("Contents", b"hi".to_vec()), ("S 1", b"there".to_vec())]);
    let record = entry_record(&f, "Contents");
    // Entry 1's right sibling points back at entry 1 (ids count the root as 0).
    set_u32(&mut f, record + 72, 1);
    assert!(matches!(Compound::new(&f), Err(Error::TreeCycle | Error::BadHeader(_) | Error::Truncated)));
}

#[test]
fn a_stream_larger_than_the_cap_is_refused() {
    let mut f = write(&[("Contents", b"hi".to_vec())]);
    let record = entry_record(&f, "Contents");
    let big = (256u64 << 20) + 1;
    f[record + 120..record + 128].copy_from_slice(&big.to_le_bytes());
    assert!(matches!(Compound::new(&f), Err(Error::StreamTooLarge | Error::Truncated | Error::ChainCycle)));
}

#[test]
fn the_size_cap_matches_the_adr_0054_profile() {
    assert_eq!(MAX_FILE, 1 << 30);
}

#[test]
fn stream_names_with_spaces_and_digits_round_trip() {
    let payloads = vec![("S 1 1724827326", b"ok".to_vec())];
    let bytes = write(&payloads);
    let f = Compound::new(&bytes).expect("opens");
    assert_eq!(f.entries()[1].path, "S 1 1724827326");
    assert_eq!(f.stream("S 1 1724827326").expect("reads"), b"ok".to_vec());
}

/// A minimal legacy document's `Contents`: the version byte, then the `CDocumentPage`
/// record — the page's stream name, then the scene's label — all in the observed record
/// encoding (`ff fe ff`-marked UTF-16 strings).
pub(crate) fn synthetic_contents() -> Vec<u8> {
    let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect() };
    let mut c = vec![0x3F, 0x01, 0x00, 0x00, 0x00];
    c.extend_from_slice(&[0xFF, 0xFF, 0x01, 0x00]); // the record stream's marker
    c.extend_from_slice(&13u16.to_le_bytes()); // the record's name length
    c.extend_from_slice(b"CDocumentPage");
    c.push(0x17); // the page record's marker
    c.push(0x00); // its attribute byte
    c.extend_from_slice(&utf16("P 1 1")); // the page's stream name (marker-terminated)
    c.extend_from_slice(&[0xFF, 0xFE, 0xFF, 0x07]); // the scene label: marker + length
    c.extend_from_slice(&utf16("Scene 1"));
    // A library item: the stream-name string (`01 80 17` marker) then its label.
    c.extend_from_slice(&[0x01, 0x80, 0x17, 0x08]);
    c.extend_from_slice(&utf16("Symbol 9"));
    c.extend_from_slice(&[0xFF, 0xFE, 0xFF, 0x08]);
    c.extend_from_slice(&utf16("UFO mask"));
    c.extend_from_slice(&9u32.to_le_bytes()); // its item id
    c.push(0x00); // a graphic
    c.extend_from_slice(&[0u8; 8]);
    c
}

/// A minimal page stream: two layers (stored bottom-first), each with one empty keyframe.
fn synthetic_page() -> Vec<u8> {
    let mut w = Ar::new();
    w.page(|w| {
        for name in ["b", "a"] {
            w.layer(name, 0, None, |w| w.frame(&[], &FrameSpec { duration: 1, ..FrameSpec::default() }, |_| {}));
        }
    });
    w.out
}

#[test]
fn a_synthetic_legacy_document_opens_with_its_scenes_and_layers() {
    use super::doc;
    let payloads = vec![
        ("Contents", synthetic_contents()),
        ("P 1 1", synthetic_page()),
        ("Symbol 9", one_frame_stream(&[])),
    ];
    let bytes = write(&payloads);
    let (document, report) = doc::import(&bytes).expect("the synthetic legacy document opens");
    assert_eq!(document.scenes.len(), 1);
    assert_eq!(document.scenes[0].name, "Scene 1");
    // Layers come bottom-first in the stream; the timeline is top-first.
    let names: Vec<&str> = document.scenes[0].timeline.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b"]);
    // Everything in it decoded: nothing to report.
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn an_unsupported_version_is_refused_with_the_specific_sentence() {
    use super::doc;
    let mut contents = synthetic_contents();
    contents[0] = 0x2F; // not a Flash 8–CS4 era version byte
    let payloads = vec![("Contents", contents), ("P 1 1", synthetic_page())];
    let bytes = write(&payloads);
    let error = doc::import(&bytes).expect_err("an unsupported version is refused");
    let message = error.to_string();
    assert!(message.contains("unsupported Flash version"), "{message}");
}

/// A point as the edge grammar stores it, relative to the pen (`crate::shape`): the three
/// encodings of the flag byte's 2-bit point fields.
#[derive(Clone, Copy)]
enum Pt {
    /// `01`: an i16 pair in 1/256 twip.
    Fine(i16, i16),
    /// `10`: an i32 pair in 1/256 twip.
    Wide(i32, i32),
    /// `11`: an i16 pair in 1/2 twip.
    Half(i16, i16),
}

impl Pt {
    fn code(self) -> u8 {
        match self {
            Pt::Fine(..) => 1,
            Pt::Wide(..) => 2,
            Pt::Half(..) => 3,
        }
    }
    fn write(self, d: &mut Vec<u8>) {
        match self {
            Pt::Fine(x, y) | Pt::Half(x, y) => {
                d.extend_from_slice(&x.to_le_bytes());
                d.extend_from_slice(&y.to_le_bytes());
            }
            Pt::Wide(x, y) => {
                d.extend_from_slice(&x.to_le_bytes());
                d.extend_from_slice(&y.to_le_bytes());
            }
        }
    }
}

/// A style change on an edge record: three u8 indices (`11`) or three u16 (`01`).
#[derive(Clone, Copy)]
enum Styles {
    Narrow(u8, u8, u8),
    Wide(u16, u16, u16),
}

/// Writes one edge record: the flag byte (move, control, anchor, style fields, low to
/// high), then the style indices and the points in that order.
fn edge(d: &mut Vec<u8>, styles: Option<Styles>, mv: Option<Pt>, ctrl: Option<Pt>, anchor: Option<Pt>) {
    let code = |p: Option<Pt>| p.map_or(0, Pt::code);
    let style_code = match styles {
        None => 0,
        Some(Styles::Wide(..)) => 1,
        Some(Styles::Narrow(..)) => 3,
    };
    d.push(code(mv) | code(ctrl) << 2 | code(anchor) << 4 | style_code << 6);
    match styles {
        Some(Styles::Narrow(a, b, c)) => d.extend_from_slice(&[a, b, c]),
        Some(Styles::Wide(a, b, c)) => {
            for v in [a, b, c] {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        None => {}
    }
    for p in [mv, ctrl, anchor].into_iter().flatten() {
        p.write(d);
    }
}

/// The start of a named `CPicShape` object as observed: the class's name, the 15-byte
/// object header (`04` … `03`), the matrix (a, b, c, d in 16.16; tx, ty in twips), the
/// `05` byte and the edge count.
fn shape_head(matrix: [i32; 6], edge_count: u32) -> Vec<u8> {
    let mut d = records::ITEM_PROLOGUE.to_vec();
    d.extend_from_slice(&[0xFF, 0xFF, 0x01, 0x00, 0x09, 0x00]); // a new class, schema 1
    d.extend_from_slice(b"CPicShape");
    d.extend_from_slice(&[0x04, 0x00, 0, 0, 0, 0, 0x00, 0x80, 0, 0, 0x00, 0x80, 0x01, 0x00, 0x03]);
    for v in matrix {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.push(0x05);
    d.extend_from_slice(&edge_count.to_le_bytes());
    d
}

/// The layer-record tail that follows a shape in a library stream.
const TAIL: [u8; 9] = [0x00, 0x80, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x03];

/// A `CPicShape` record holding the corpus's smallest shape's geometry (Symbol 123, "UFO
/// mask"): a 10.9 × 11.2 px rectangle, white fill at 31 % opacity, its matrix scaled by
/// 65535/65536 and translated to (26.9, −1.55). The bytes are written from the decoded
/// grammar (ADR 0096), not copied from any file.
fn synthetic_shape_record() -> Vec<u8> {
    let mut d = shape_head([0xFFFF, 0, 0, 0xFFFF, 538, -31], 4);
    d.extend_from_slice(&1u16.to_le_bytes()); // one fill:
    d.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0x50, 0x00, 0x00]); // white at 80/255, solid
    d.extend_from_slice(&0u16.to_le_bytes()); // no strokes
    // The right-hand fill 1, a move to (−429, −81) twips, then four straight edges.
    edge(&mut d, Some(Styles::Narrow(0, 0, 1)), Some(Pt::Half(-858, -162)), None, Some(Pt::Half(0, 448)));
    edge(&mut d, None, None, None, Some(Pt::Half(-436, 0)));
    edge(&mut d, None, None, None, Some(Pt::Half(0, -448)));
    edge(&mut d, None, None, None, Some(Pt::Half(436, 0)));
    d.push(0x00); // the edge list's end
    d.extend_from_slice(&TAIL);
    d
}

/// The body of a `CPicShape` record of three 10 × 10 px squares side by side, each in its own
/// solid fill (red, green, blue): one record that emits three shapes.
pub(crate) fn three_squares_body() -> Vec<u8> {
    let mut d = shape_head([0x10000, 0, 0, 0x10000, 0, 0], 12);
    d.extend_from_slice(&3u16.to_le_bytes());
    for rgb in [[0xFF, 0, 0], [0, 0xFF, 0], [0, 0, 0xFF]] {
        d.extend_from_slice(&rgb);
        d.extend_from_slice(&[0xFF, 0x00, 0x00]); // opaque, solid
    }
    d.extend_from_slice(&0u16.to_le_bytes()); // no strokes
    for fill in 1..=3u8 {
        // A move 15 px along from the last square's corner (the first from the origin), then
        // the square, back to that corner.
        let mv = if fill == 1 { Pt::Half(0, 0) } else { Pt::Half(600, 0) };
        edge(&mut d, Some(Styles::Narrow(0, 0, fill)), Some(mv), None, Some(Pt::Half(0, 400)));
        edge(&mut d, None, None, None, Some(Pt::Half(400, 0)));
        edge(&mut d, None, None, None, Some(Pt::Half(0, -400)));
        edge(&mut d, None, None, None, Some(Pt::Half(-400, 0)));
    }
    d.push(0x00); // the edge list's end
    d.extend_from_slice(&TAIL);
    record_body(&d)
}

fn library_shapes(record: Vec<u8>) -> (Vec<std::sync::Arc<anicraft_doc::Element>>, super::Report) {
    use super::doc;
    let payloads = vec![("Contents", synthetic_contents()), ("P 1 1", synthetic_page()), ("Symbol 9", one_frame_stream(&record_body(&record)))];
    let (document, report) = doc::import(&write(&payloads)).expect("opens");
    let elements = document.library.iter().find(|s| s.name == "UFO mask")
        .map(|symbol| symbol.timeline.layers[0].keyframes[0].elements.clone())
        .unwrap_or_default();
    (elements, report)
}

/// A shape record's body (from its matrix to the edge list's end), as a keyframe holds it.
fn record_body(record: &[u8]) -> Vec<u8> {
    let start = records::ITEM_PROLOGUE.len() + 6 + 9 + 15;
    // Up to the edge list's end when it decodes (the tail after it is the library object's).
    let end = super::shape::find_all(record).first().map_or(record.len(), |r| r.end);
    record.get(start..end).map(<[u8]>::to_vec).unwrap_or_default()
}

/// An item stream of one layer with one keyframe whose raw artwork is `body`.
pub(crate) fn one_frame_stream(body: &[u8]) -> Vec<u8> {
    let mut w = Ar::new();
    w.page(|w| {
        w.layer("artwork", 0, None, |w| {
            w.frame(body, &FrameSpec { duration: 1, ..FrameSpec::default() }, |_| {});
        });
    });
    w.out
}

fn shape_of(element: &anicraft_doc::Element) -> &anicraft_doc::Shape {
    match &element.kind {
        anicraft_doc::ElementKind::Shape(shape) => shape,
        other => panic!("expected a shape, got {other:?}"),
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn a_straight_edged_shape_decodes_with_its_geometry_and_fill() {
    let (elements, report) = library_shapes(synthetic_shape_record());
    assert_eq!(elements.len(), 1, "the rectangle decodes as one shape");
    let shape = shape_of(&elements[0]);
    // The fill: white at 80/255.
    let Some(anicraft_doc::Paint::Solid(fill)) = shape.fill else { panic!("a solid fill") };
    assert!((fill.a - 80.0 / 255.0).abs() < 0.001, "fill alpha {fill:?}");
    // The matrix: 65535/65536 scale, translation (26.9, −1.55) px.
    let m = elements[0].transform.m;
    assert!(close(m[0], 65535.0 / 65536.0) && close(m[3], 65535.0 / 65536.0) && m[1] == 0.0 && m[2] == 0.0, "{m:?}");
    assert!(close(m[4], 26.9) && close(m[5], -1.55), "{m:?}");
    // The path: the rectangle's corners, (−429, −81) … (−647, 143) twips, as one closed loop.
    let points: Vec<_> = shape.path.cmds.iter().filter_map(|c| match *c {
        anicraft_geom::PathCmd::MoveTo(p) | anicraft_geom::PathCmd::LineTo(p) => Some((p.x, p.y)),
        anicraft_geom::PathCmd::Close => None,
        _ => panic!("straight edges only"),
    }).collect();
    let want = [(-21.45, -4.05), (-21.45, 7.15), (-32.35, 7.15), (-32.35, -4.05), (-21.45, -4.05)];
    assert_eq!(points.len(), want.len(), "{points:?}");
    for (got, want) in points.iter().zip(want) {
        assert!(close(got.0, want.0) && close(got.1, want.1), "{points:?}");
    }
    assert_eq!(shape.path.cmds.last(), Some(&anicraft_geom::PathCmd::Close));
    assert!(report.skipped.is_empty(), "a fully decoded shape skips nothing: {:?}", report.skipped);
}

/// A shape exercising the whole edge grammar: a radial gradient fill (with a focal point),
/// a solid fill, a square-capped stroke; quadratic edges in all three point encodings; a
/// style change alone; a move; u16 style indices with their flag bit; a region drawn with
/// its fill on the left. Coordinates in twips: a rounded loop from (100, 100) through
/// (140, 140), (100, 180) and (60, 140), then a triangle (300, 100), (400, 200), (300, 200).
fn synthetic_curve_record() -> Vec<u8> {
    let mut d = shape_head([65536, 0, 0, 65536, 0, 0], 7);
    d.extend_from_slice(&2u16.to_le_bytes()); // two fills:
    d.extend_from_slice(&[0, 0, 0, 0, 0x12, 0x00]); // 1: radial gradient
    for v in [65536, 0, 0, 65536, 100, 0] {
        d.extend_from_slice(&i32::to_le_bytes(v)); // its matrix: (5, 0) px
    }
    d.push(2); // two stops
    d.extend_from_slice(&(-51i32).to_le_bytes()); // focal point −51/255 = −0.2
    d.extend_from_slice(&[0; 4]);
    d.extend_from_slice(&[0x00, 0xFF, 0x00, 0x00, 0xFF]); // red at 0
    d.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF, 0xFF]); // blue at 1
    d.extend_from_slice(&[0x00, 0x80, 0x00, 0xFF, 0x00, 0x37]); // 2: solid green (the second byte varies)
    d.extend_from_slice(&1u16.to_le_bytes()); // one stroke:
    d.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF]); // black
    d.extend_from_slice(&30u16.to_le_bytes()); // 1.5 px
    d.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0x02, 0x00, 0x00, 0x05]); // square caps, miter joins, limit 5
    d.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF, 0x00, 0xD9]); // its fill: the colour again
    let fine = |tw: i16| tw * 256; // twips in 1/256 twip
    let wide = |tw: i32| tw * 256;
    let half = |tw: i16| tw * 2;
    // Stroke 1 and fill 1 on the right: a move, then a curve with 01-encoded points.
    edge(&mut d, Some(Styles::Narrow(1, 0, 1)), Some(Pt::Half(half(100), half(100))), Some(Pt::Fine(fine(40), 0)), Some(Pt::Fine(fine(40), fine(40))));
    // A 11-encoded control point and a 10-encoded anchor.
    edge(&mut d, None, None, Some(Pt::Half(0, half(40))), Some(Pt::Wide(wide(-40), wide(40))));
    // The reverse.
    edge(&mut d, None, None, Some(Pt::Wide(wide(-40), 0)), Some(Pt::Half(half(-40), half(-40))));
    // A straight edge back to the start.
    edge(&mut d, None, None, None, Some(Pt::Fine(fine(40), fine(-40))));
    // A style change alone, then u16 indices (fill 2 on the left, flagged) with a move.
    edge(&mut d, Some(Styles::Narrow(0, 0, 0)), None, None, None);
    edge(&mut d, Some(Styles::Wide(0, 0x8002, 0)), Some(Pt::Half(half(200), 0)), None, Some(Pt::Half(half(100), half(100))));
    edge(&mut d, None, None, None, Some(Pt::Half(half(-100), 0)));
    edge(&mut d, None, None, None, Some(Pt::Half(0, half(-100))));
    d.push(0x00);
    d.extend_from_slice(&TAIL);
    d
}

#[test]
fn the_edge_grammar_decodes_curves_styles_and_every_point_encoding() {
    let data = synthetic_curve_record();
    let records = super::shape::find_all(&data);
    assert_eq!(records.len(), 1, "one shape body");
    let r = &records[0];
    let v = |x: f32, y: f32| anicraft_geom::Vec2::new(x / 20.0, y / 20.0); // twips to px
    let seg = |from, ctrl, to| super::shape::Seg { from, ctrl, to };
    let segs: Vec<_> = r.edges.iter().map(|e| (e.stroke, e.fill_left, e.fill_right, e.seg)).collect();
    assert_eq!(segs, vec![
        (1, 0, 1, seg(v(100.0, 100.0), Some(v(140.0, 100.0)), v(140.0, 140.0))),
        (1, 0, 1, seg(v(140.0, 140.0), Some(v(140.0, 180.0)), v(100.0, 180.0))),
        (1, 0, 1, seg(v(100.0, 180.0), Some(v(60.0, 180.0)), v(60.0, 140.0))),
        (1, 0, 1, seg(v(60.0, 140.0), None, v(100.0, 100.0))),
        (0, 2, 0, seg(v(300.0, 100.0), None, v(400.0, 200.0))),
        (0, 2, 0, seg(v(400.0, 200.0), None, v(300.0, 200.0))),
        (0, 2, 0, seg(v(300.0, 200.0), None, v(300.0, 100.0))),
    ]);
    assert!(r.approximations.is_empty(), "{:?}", r.approximations);
}

#[test]
fn a_curved_shape_imports_as_fill_regions_and_strokes() {
    use anicraft_geom::{PathCmd, Vec2};
    let (elements, report) = library_shapes(synthetic_curve_record());
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(elements.len(), 3, "two fill regions, then one stroke");
    let p = |x: f32, y: f32| Vec2::new(x / 20.0, y / 20.0);
    // Fill 1: the rounded loop, its curves as quadratics.
    let first = shape_of(&elements[0]);
    assert_eq!(first.path.cmds, vec![
        PathCmd::MoveTo(p(100.0, 100.0)),
        PathCmd::QuadTo(p(140.0, 100.0), p(140.0, 140.0)),
        PathCmd::QuadTo(p(140.0, 180.0), p(100.0, 180.0)),
        PathCmd::QuadTo(p(60.0, 180.0), p(60.0, 140.0)),
        PathCmd::LineTo(p(100.0, 100.0)),
        PathCmd::Close,
    ]);
    let Some(anicraft_doc::Paint::Radial { stops, focal, .. }) = &first.fill else { panic!("a radial gradient: {:?}", first.fill) };
    assert!(close(*focal, -0.2), "focal {focal}");
    assert_eq!(stops.len(), 2);
    assert!(close(stops[0].offset, 0.0) && close(stops[1].offset, 1.0));
    assert!(close(stops[0].color.r, 1.0) && close(stops[1].color.b, 1.0));
    // Fill 2 lies on the edges' left: its loop runs the triangle backwards.
    let second = shape_of(&elements[1]);
    assert_eq!(second.path.cmds, vec![
        PathCmd::MoveTo(p(400.0, 200.0)),
        PathCmd::LineTo(p(300.0, 100.0)),
        PathCmd::LineTo(p(300.0, 200.0)),
        PathCmd::LineTo(p(400.0, 200.0)),
        PathCmd::Close,
    ]);
    let Some(anicraft_doc::Paint::Solid(green)) = second.fill else { panic!("a solid fill") };
    assert!(close(green.g, 128.0 / 255.0) && close(green.r, 0.0));
    // The stroke: the loop's edges in drawing order, with the decoded style.
    let third = shape_of(&elements[2]);
    let stroke = third.stroke.as_ref().expect("a stroke");
    assert!(close(stroke.width, 1.5));
    assert_eq!((stroke.style.cap, stroke.style.join), (anicraft_doc::Cap::Square, anicraft_doc::Join::Miter));
    assert!(close(stroke.style.miter_limit, 5.0));
    assert!(third.fill.is_none());
    assert_eq!(third.path.cmds.len(), 5, "a move and the four edges: {:?}", third.path.cmds);
    assert!(matches!(third.path.cmds[1], PathCmd::QuadTo(..)));
}

/// A stroke's ten style bytes from the grammar: dash and gap fields, cap, join, miter limit.
fn stroke_style_bytes(dash: u16, gap: u16) -> [u8; 10] {
    let (d, g) = (dash.to_le_bytes(), gap.to_le_bytes());
    [d[0], d[1], g[0], g[1], 0, 0, 0x01, 0x01, 0x00, 0x03]
}

#[test]
fn stroke_style_bytes_read_as_their_kind_and_pattern() {
    use super::shape::StrokeKind::{self, *};
    let of = |dash, field| StrokeKind::of(&stroke_style_bytes(dash, field));
    assert_eq!(of(0, 0), Solid);
    // A dash length: dashed, both lengths in twips (6 and 6 px is Animate's default).
    assert_eq!(of(120, 120), Dashed { dash: 6.0, gap: 6.0 });
    assert_eq!(of(120, 180), Dashed { dash: 6.0, gap: 9.0 });
    // Otherwise the low four bits are the type: 2 dotted, its spacing in tenths of a pixel.
    assert_eq!(of(0, (30 << 4) | 2), Dotted { spacing: 3.0 });
    assert_eq!(of(0, (45 << 4) | 2), Dotted { spacing: 4.5 });
    assert_eq!(of(0, (7 << 4) | 3), Ragged);
    assert_eq!(of(0, (7 << 4) | 4), Stipple);
    assert_eq!(of(0, (7 << 4) | 5), Hatched);
    // Types the corpus doesn't show, and a solid type with a pattern, aren't guessed at.
    for field in [1, 6, 0xF, 0x10, (3 << 4) | 1] {
        assert_eq!(of(0, field), Unknown, "{field:#x}");
    }
    // Bytes 4 and 8 are zero in every known stroke: anything else isn't decoded yet.
    for at in [4, 8] {
        let mut style = stroke_style_bytes(120, 120);
        style[at] = 1;
        assert_eq!(StrokeKind::of(&style), Unknown, "byte {at}");
    }
}

#[test]
fn stroke_kinds_draw_as_dashes_dots_or_solid_lines_with_a_note() {
    use super::shape::StrokeKind::*;
    use anicraft_doc::Dash;
    assert_eq!(Solid.dash(), (None, None));
    assert_eq!(Dashed { dash: 6.0, gap: 9.0 }.dash(), (Dash::dashed(6.0, 9.0), None));
    assert_eq!(Dotted { spacing: 3.0 }.dash(), (Dash::dotted(3.0), None));
    // A dash with no gap is a solid line.
    assert_eq!(Dashed { dash: 6.0, gap: 0.0 }.dash(), (None, None));
    // The largest fields stay drawable (the model clamps the lengths).
    assert!(super::shape::StrokeKind::of(&stroke_style_bytes(u16::MAX, u16::MAX)).dash().0.is_some());
    assert!(super::shape::StrokeKind::of(&stroke_style_bytes(0, 0xFFF2)).dash().0.is_some());
    for kind in [Ragged, Stipple, Hatched] {
        assert_eq!(kind.dash(), (None, Some("ragged, stipple and hatched strokes (drawn solid)")));
    }
    assert_eq!(Unknown.dash(), (None, Some("stroke styles not decoded yet (drawn solid)")));
}

/// Where the synthetic curve record's stroke style bytes start: after the radial and solid
/// fills, the stroke count, the stroke's RGBA and width.
fn curve_record_stroke_style(data: &[u8]) -> usize {
    let body = super::shape::body_after_tag(data, records::ITEM_PROLOGUE.len() + 6 + 9).expect("the body");
    let fill2 = body + 7 + 6 + 24 + 9 + 10;
    fill2 + 6 + 2 + 4 + 2
}

#[test]
fn dashed_and_dotted_strokes_import_with_their_pattern() {
    use anicraft_doc::Dash;
    let good = synthetic_curve_record();
    let at = curve_record_stroke_style(&good);
    assert_eq!(&good[at..at + 10], &[0, 0, 0, 0, 0, 0, 0x02, 0x00, 0x00, 0x05], "the square-capped stroke");
    let stroke_of = |data: Vec<u8>| {
        let (elements, report) = library_shapes(data);
        assert_eq!(elements.len(), 3, "two fill regions, then one stroke");
        (shape_of(&elements[2]).stroke.clone().expect("a stroke"), report)
    };
    // The solid original stays solid.
    assert_eq!(stroke_of(good.clone()).0.style.dash, None);
    // Dashed, 8 px on and 2.5 px off, its caps, joins and miter limit kept.
    let mut style = [0u8; 10];
    style.copy_from_slice(&good[at..at + 10]);
    style[..4].copy_from_slice(&[160, 0, 50, 0]);
    let (stroke, report) = stroke_of(patch(good.clone(), at, &style));
    assert_eq!(stroke.style.dash, Dash::dashed(8.0, 2.5));
    assert_eq!((stroke.style.cap, stroke.style.join), (anicraft_doc::Cap::Square, anicraft_doc::Join::Miter));
    assert!(close(stroke.style.miter_limit, 5.0));
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    // Dotted, 5 px apart.
    style[..4].copy_from_slice(&[0, 0, 0x22, 0x03]); // (50 << 4) | 2
    let (stroke, report) = stroke_of(patch(good.clone(), at, &style));
    assert_eq!(stroke.style.dash, Dash::dotted(5.0));
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    // Hatched: solid, counted as the XFL importer counts it.
    style[..4].copy_from_slice(&[0, 0, 0x75, 0x00]);
    let (stroke, report) = stroke_of(patch(good.clone(), at, &style));
    assert_eq!(stroke.style.dash, None);
    assert_eq!(report.skipped.get("ragged, stipple and hatched strokes (drawn solid)"), Some(&1), "{:?}", report.skipped);
    // A style not decoded yet draws solid, and the import counts it.
    style[4] = 0x01;
    let (stroke, report) = stroke_of(patch(good, at, &style));
    assert_eq!(stroke.style.dash, None);
    assert_eq!(report.skipped.get("stroke styles not decoded yet (drawn solid)"), Some(&1), "{:?}", report.skipped);
}

/// `data` with `to` written over it at `at`.
fn patch(mut data: Vec<u8>, at: usize, to: &[u8]) -> Vec<u8> {
    data[at..at + to.len()].copy_from_slice(to);
    data
}

#[test]
fn edge_records_the_grammar_does_not_cover_leave_the_shape_out_and_are_counted() {
    use super::shape::{decode, find_all, ShapeError};
    let good = synthetic_curve_record();
    let body = super::shape::body_after_tag(&good, records::ITEM_PROLOGUE.len() + 6 + 9).expect("the body");
    assert!(decode(&good, body).is_ok());
    // The first edge record's flag: after the fills and the stroke.
    let flag_at = good.iter().enumerate().skip(body).find(|&(i, _)| good[i..].starts_with(&[0x00, 0x00, 0x00, 0xFF, 0x00, 0xD9])).map(|(i, _)| i + 6).expect("the stroke's fill");
    assert_eq!(good[flag_at] >> 6, 3, "the first record changes styles");
    // Style field `10`: a width the corpus never shows.
    let bad = patch(good.clone(), flag_at, &[(good[flag_at] & 0x3F) | 0x80]);
    assert_eq!(decode(&bad, body).err(), Some(ShapeError::StyleWidth));
    // A control point without an anchor.
    let bad = patch(good.clone(), flag_at, &[0x0C]);
    assert!(decode(&bad, body).is_err());
    // An edge count the list doesn't match.
    let bad = patch(good.clone(), body + 1, &8u32.to_le_bytes());
    assert_eq!(decode(&bad, body).err(), Some(ShapeError::Count));
    let bad = patch(good.clone(), body + 1, &6u32.to_le_bytes());
    assert_eq!(decode(&bad, body).err(), Some(ShapeError::Count));
    // A bitmap fill (layout not decoded) leaves the shape out, counted by kind.
    let fill2 = body + 7 + 6 + 24 + 9 + 10;
    assert_eq!(&good[fill2 + 4..fill2 + 6], &[0x00, 0x37]);
    let bad = patch(good.clone(), fill2 + 4, &[0x41]);
    assert_eq!(decode(&bad, body).err(), Some(ShapeError::FillKind(0x41)));
    assert!(find_all(&bad).is_empty());
    let (elements, report) = library_shapes(bad);
    assert!(elements.is_empty());
    assert_eq!(report.skipped.get("shapes with bitmap fills (decode pending)"), Some(&1), "{:?}", report.skipped);
    let (elements, report) = library_shapes(patch(good.clone(), flag_at, &[0x0C]));
    assert!(elements.is_empty());
    assert_eq!(report.skipped.get("shapes whose edge records didn't decode"), Some(&1), "{:?}", report.skipped);
}

#[test]
fn a_shape_record_cut_short_anywhere_never_panics_and_never_half_draws() {
    let good = synthetic_curve_record();
    let full_end = super::shape::find_all(&good)[0].end;
    for cut in 0..good.len() {
        let (elements, report) = library_shapes(good[..cut].to_vec());
        if cut < full_end {
            assert!(elements.is_empty(), "cut at {cut}: a partial record drew {} shapes", elements.len());
            // (A cut before the body's start leaves the keyframe an empty body: nothing lost.)
            if cut > records::ITEM_PROLOGUE.len() + 6 + 9 + 15 {
                assert!(!report.skipped.is_empty(), "cut at {cut}: the left-out shape is counted");
            }
        } else {
            assert_eq!(elements.len(), 3, "cut at {cut}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Timelines: an independent writer for the item streams' object graph (ADR 0096), written from
// the decoded grammar (`crate::archive`), never from corpus bytes.

/// Writes an item stream: the `01` byte, then objects the way the format numbers them (classes
/// and objects share one count from 1; a new class is declared by name, a known one referenced
/// as `0x8000 | n`, an object written before as `n`).
pub(crate) struct Ar {
    pub(crate) out: Vec<u8>,
    classes: Vec<(&'static str, u16)>,
    pub(crate) count: u16,
}

/// What a keyframe record carries besides its artwork and objects.
#[derive(Clone, Default)]
pub(crate) struct FrameSpec {
    pub(crate) duration: u16,
    /// 0 none, 1 classic, 2 shape.
    tween: u8,
    ease: i16,
    label: &'static str,
    comment: bool,
    /// Custom ease anchors for the `all` group: (in, anchor, out) points.
    ease_all: Vec<[f64; 6]>,
    script: &'static str,
    /// The sound's media id (0: none), its envelope (mark44, left, right), loops, sync, in
    /// and out points.
    sound: u16,
    envelope: Vec<(u32, u16, u16)>,
    loops: u16,
    sync: u8,
    in_out: (u32, u32),
    /// Shape hints: (start, end) in twips.
    hints: Vec<((i32, i32), (i32, i32))>,
}

/// A symbol instance as written.
#[derive(Clone)]
pub(crate) struct InstSpec {
    pub(crate) class: &'static str,
    item: u16,
    matrix: [i32; 6],
    /// The transformation point placed on the stage, in twips.
    point: (i32, i32),
    pub(crate) first_frame: u16,
    loop_mode: u16,
    /// (style, A mul, A add, R mul, R add, G mul, G add, B mul, B add, percent, tint RGBA)
    color: (u8, [i16; 8], i16, [u8; 4]),
    blend: u8,
    flag: u8,
    persistent: Vec<(&'static str, &'static str)>,
}

impl Default for InstSpec {
    fn default() -> Self {
        InstSpec {
            class: "CPicSymbol",
            item: 9,
            matrix: [65536, 0, 0, 65536, 0, 0],
            point: (0, 0),
            first_frame: 0,
            loop_mode: 0,
            color: (0, [256, 0, 256, 0, 256, 0, 256, 0], 0, [255, 255, 255, 255]),
            blend: 1,
            flag: 0,
            persistent: Vec::new(),
        }
    }
}

impl Ar {
    pub(crate) fn new() -> Ar {
        Ar { out: vec![0x01], classes: Vec::new(), count: 1 }
    }
    pub(crate) fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn string(&mut self, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.out.extend_from_slice(&[0xFF, 0xFE, 0xFF]);
        if units.len() < 0xFF {
            self.out.push(units.len() as u8);
        } else {
            self.out.push(0xFF);
            self.u16(units.len() as u16);
        }
        for u in units {
            self.u16(u);
        }
    }
    /// An object's tag; returns the object's number.
    pub(crate) fn tag(&mut self, class: &'static str) -> u16 {
        if let Some(&(_, n)) = self.classes.iter().find(|(c, _)| *c == class) {
            self.u16(0x8000 | n);
        } else {
            self.out.extend_from_slice(&[0xFF, 0xFF, 0x01, 0x00]);
            self.u16(class.len() as u16);
            self.out.extend_from_slice(class.as_bytes());
            self.classes.push((class, self.count));
            self.count += 1;
        }
        self.count += 1;
        self.count - 1
    }
    /// A `CPic*` object's head, its children, then its base fields.
    pub(crate) fn object(&mut self, class: &'static str, flag: u8, point: Option<(i32, i32)>, word: u16, kind: u8, children: impl FnOnce(&mut Ar)) -> u16 {
        let n = self.tag(class);
        self.out.extend_from_slice(&[0x04, flag]);
        children(self);
        self.out.extend_from_slice(&[0x00, 0x00]);
        let (x, y) = point.unwrap_or((i32::MIN, i32::MIN));
        self.i32(x);
        self.i32(y);
        self.u16(word);
        self.out.push(kind);
        n
    }
    pub(crate) fn page(&mut self, layers: impl FnOnce(&mut Ar)) {
        self.object("CPicPage", 0, None, 0, 0x07, layers);
        self.out.extend_from_slice(&[0x02, 0x00, 0x01, 0x00, 0, 0, 0, 0]);
        self.i32(0); // no ruler guides
    }
    /// A layer; `parent` is the layer it sits under, written in place (as the first child of a
    /// folder has it).
    pub(crate) fn layer(&mut self, name: &str, kind: u8, parent: Option<Parent>, frames: impl FnOnce(&mut Ar)) -> u16 {
        let n = self.object("CPicLayer", 0, None, 0, 0x0B, frames);
        self.layer_fields(name, kind, parent);
        n
    }
    fn layer_fields(&mut self, name: &str, kind: u8, parent: Option<Parent>) {
        self.string(name);
        self.out.extend_from_slice(&[0, 0, 1]); // not current, not hidden, locked
        self.i32(-1);
        self.out.extend_from_slice(&[0x4F, 0xFF, 0x4F, 0xFF]);
        self.i32(0);
        self.i32(1);
        self.out.push(kind);
        match parent {
            None => self.u16(0),
            Some(Parent::Inline(name, kind)) => {
                self.object("CPicLayer", 0, None, 0, 0x0B, |_| {});
                self.layer_fields(name, kind, None);
            }
        }
        self.out.extend_from_slice(&[0x01, 0x00]);
    }
    /// A keyframe: its objects, its raw artwork (a shape body), then its fields.
    pub(crate) fn frame(&mut self, body: &[u8], spec: &FrameSpec, objects: impl FnOnce(&mut Ar)) {
        self.object("CPicFrame", 0, None, 0, 0x03, objects);
        if body.is_empty() {
            self.empty_shape();
        } else {
            self.out.extend_from_slice(body);
        }
        self.i32(0); // no cubic hints
        self.out.push(0x18);
        self.u16(spec.duration);
        self.out.extend_from_slice(&[spec.tween, 0x06]);
        self.out.extend_from_slice(&spec.ease.to_le_bytes());
        self.u16(spec.sound);
        self.u16(spec.envelope.len() as u16);
        for &(mark, left, right) in &spec.envelope {
            self.i32(mark as i32);
            self.u16(left);
            self.u16(right);
        }
        self.u16(spec.loops.max(1));
        self.out.push(spec.sync);
        let (in_point, out_point) = if spec.sound == 0 { (0, 0x3FFF_FFFF) } else { spec.in_out };
        self.i32(in_point as i32);
        self.i32(out_point as i32);
        self.u16(0xFFFF);
        self.string(spec.label);
        self.out.extend_from_slice(&[4, 0, 0, 0, 1, 0, 0, 0, 0x33, 0x0D, 0, 0, 0, 0, 0, 0]);
        self.string(spec.script);
        self.out.extend_from_slice(&[0; 8]);
        self.i32(spec.comment as i32);
        self.u16(0); // no shape-tween cache
        self.out.extend_from_slice(&[0; 4]);
        if spec.hints.is_empty() {
            self.u16(0); // no hints
        } else {
            // A `CObList` (schema 0) of `CMorphHintItem`s.
            self.tag("CObList");
            self.u16(spec.hints.len() as u16);
            for (n, &((sx, sy), (ex, ey))) in spec.hints.iter().enumerate() {
                self.tag("CMorphHintItem");
                for v in [sx, sy, ex, ey, n as i32] {
                    self.i32(v);
                }
            }
        }
        self.string("");
        for v in [1, 0, 0, 1, (!spec.ease_all.is_empty()) as i32] {
            self.i32(v);
        }
        if !spec.ease_all.is_empty() {
            for group in 0..6 {
                if group < 5 {
                    self.i32(0);
                    continue;
                }
                self.i32(spec.ease_all.len() as i32);
                for a in &spec.ease_all {
                    for v in a {
                        self.out.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
    }
    /// A shape body without edges: the identity matrix, no styles.
    pub(crate) fn empty_shape(&mut self) {
        for v in [65536, 0, 0, 65536, 0, 0] {
            self.i32(v);
        }
        self.out.push(0x05);
        self.i32(0);
        self.u16(0);
        self.u16(0);
        self.out.push(0x00);
    }
    pub(crate) fn instance(&mut self, spec: &InstSpec) {
        self.object(spec.class, spec.flag, Some(spec.point), 0, 0x13, |_| {});
        for v in spec.matrix {
            self.i32(v);
        }
        self.u16(spec.first_frame);
        self.u16(spec.loop_mode);
        self.out.push(1);
        let (style, chans, percent, tint) = spec.color;
        for v in chans {
            self.out.extend_from_slice(&v.to_le_bytes());
        }
        self.out.extend_from_slice(&[style, 0]);
        self.out.extend_from_slice(&percent.to_le_bytes());
        self.out.extend_from_slice(&tint);
        self.string("");
        self.u16(spec.item);
        self.out.extend_from_slice(&[0, 0, 0]);
        if spec.persistent.is_empty() {
            self.out.push(0);
        } else {
            self.out.push(1);
            self.i32(1);
            self.i32(spec.persistent.len() as i32);
            for (k, v) in &spec.persistent {
                self.out.push(0x05);
                self.string(k);
                self.string(v);
            }
        }
        self.out.extend_from_slice(&[0, 0, spec.blend, 0, 0]);
    }
}

pub(crate) enum Parent {
    /// The parent layer, written in place.
    Inline(&'static str, u8),
}

/// The rectangle's body (from `synthetic_shape_record`).
pub(crate) fn rectangle_body() -> Vec<u8> {
    record_body(&synthetic_shape_record())
}

/// A scene's page: a folder written in place as its first child's parent, a guide, a layer
/// with a classic tween, a comment and a name label, a zero-length frame and a scaled,
/// tinted, single-frame instance with a moved transformation point; then the folder's own
/// place in the stack (a back-reference) and a top layer.
fn rich_page() -> (Vec<u8>, InstSpec) {
    let inst = InstSpec {
        // Scale 1.25 and −0.5, a 30° rotation's b and c, translation (100.5, −20.25) px.
        matrix: [81920, 32768, -32768, -32768, 2010, -405],
        point: (2010 + 200, -405 + 100),
        first_frame: 4,
        loop_mode: 2,
        color: (2, [256, 0, 128, 0, 128, 0, 128, 0], 50, [255, 0, 0, 255]),
        blend: 7,
        flag: 0x04,
        persistent: vec![("rigData", "{\"id\":1}")],
        ..InstSpec::default()
    };
    let mut w = Ar::new();
    let rect = rectangle_body();
    let i2 = inst.clone();
    let mut folder = 0u16;
    w.page(|w| {
        // Bottom first: a layer inside a folder (the folder written in place as its parent).
        w.layer("in folder", 0, Some(Parent::Inline("folder", 3)), |w| {
            w.frame(&rect, &FrameSpec { duration: 10, ..FrameSpec::default() }, |_| {});
        });
        // The folder was the last object written.
        folder = w.count - 1;
        w.u16(folder); // the folder's own place in the stack: a back-reference
        w.layer("animation", 0, None, |w| {
            w.frame(&[], &FrameSpec { duration: 0, ..FrameSpec::default() }, |_| {}); // dropped
            w.frame(&[], &FrameSpec { duration: 3, tween: 1, ease: -50, label: "start", ..FrameSpec::default() }, |w| w.instance(&i2));
            w.frame(&[], &FrameSpec { duration: 7, label: "note to self", comment: true, ease_all: vec![[0.0, 0.0, 0.0, 0.0, 0.25, 0.1], [0.75, 0.9, 1.0, 1.0, 1.0, 1.0]], tween: 2, ..FrameSpec::default() }, |w| {
                w.instance(&InstSpec { class: "CPicSprite", ..InstSpec::default() });
            });
        });
        w.layer("guide", 1, None, |w| w.frame(&[], &FrameSpec { duration: 10, script: "stop();", ..FrameSpec::default() }, |_| {}));
    });
    (w.out, inst)
}

/// A movie-clip instance's settings after its common fields, in a layout not decoded: a
/// version byte, the frame actions' header, two strings, then component data.
fn sprite_extras(w: &mut Ar) {
    w.out.push(0x08);
    w.out.extend_from_slice(&[4, 0, 0, 0, 1, 0, 0, 0, 0x42, 0x57, 0, 0, 0, 0, 0, 0]);
    w.string("");
    w.string("");
    w.out.extend_from_slice(&[2, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0]);
    w.string("<component metaDataFetched='true'/>");
}

fn rich_document() -> (Vec<u8>, InstSpec) {
    let (page, inst) = rich_page();
    // The movie clip in `rich_page` carries its extras: rewrite it with them in place.
    let page = with_sprite_extras(page);
    let mut symbol = Ar::new();
    let rect = rectangle_body();
    symbol.page(|w| {
        w.layer("artwork", 0, None, |w| {
            w.frame(&rect, &FrameSpec { duration: 2, ..FrameSpec::default() }, |_| {});
            w.frame(&[], &FrameSpec { duration: 3, ..FrameSpec::default() }, |_| {});
        });
    });
    let payloads = vec![("Contents", synthetic_contents()), ("P 1 1", page), ("Symbol 9", symbol.out)];
    (write(&payloads), inst)
}

/// `page` with a movie clip's extra settings after the sprite instance's common fields.
fn with_sprite_extras(page: Vec<u8>) -> Vec<u8> {
    let name = b"CPicSprite";
    let Some(at) = page.windows(name.len()).position(|w| w == name) else { return page };
    // The sprite's common fields end with its blend byte and two zero bytes: 15 bytes of base,
    // 24 of matrix, 4, 1, 16, 1, 1, 2, 4, the empty name (4), the item (2), 3 + 1 + 1 + 1 + 1 + 2.
    let end = at + name.len() + 15 + 24 + 4 + 1 + 16 + 1 + 1 + 2 + 4 + 4 + 2 + 3 + 1 + 1 + 1 + 1 + 2;
    let mut extras = Ar::new();
    extras.out.clear();
    sprite_extras(&mut extras);
    let mut out = page[..end].to_vec();
    out.extend_from_slice(&extras.out);
    out.extend_from_slice(&page[end..]);
    out
}

pub(crate) fn scene(doc: &anicraft_doc::Document) -> &anicraft_doc::Timeline {
    &doc.scenes[0].timeline
}

#[test]
fn a_synthetic_timeline_decodes_its_layers_keyframes_tweens_and_instances() {
    use anicraft_doc::{BlendMode, ColorEffect, ElementKind, LayerKind, LoopMode, TweenKind};
    let (bytes, inst) = rich_document();
    let (doc, report) = super::doc::import(&bytes).expect("the synthetic document opens");
    let tl = scene(&doc);
    // Top first: the guide, the animation, the folder, the layer in it.
    let names: Vec<&str> = tl.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["guide", "animation", "folder", "in folder"]);
    let kinds: Vec<LayerKind> = tl.layers.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, [LayerKind::Guide, LayerKind::Normal, LayerKind::Folder, LayerKind::Normal]);
    assert_eq!(tl.layers[3].parent, Some(tl.layers[2].id), "the layer sits in the folder");
    assert!(tl.layers[2].keyframes.is_empty(), "a folder holds no frames");
    assert!(tl.layers[0].locked && tl.layers[0].visible);
    // The animation's keyframes: the zero-length record dropped, the rest one after another.
    let anim = &tl.layers[1];
    let spans: Vec<(u32, u32)> = anim.keyframes.iter().map(|k| (k.start, k.duration)).collect();
    assert_eq!(spans, [(0, 3), (3, 7)]);
    let first = &anim.keyframes[0];
    assert_eq!(first.label, "start");
    let tween = first.tween.as_ref().expect("a classic tween");
    assert_eq!(tween.kind, TweenKind::Classic);
    assert_eq!(tween.easing, anicraft_doc::Easing::Classic(-50.0));
    let second = &anim.keyframes[1];
    assert_eq!(second.label, "", "a comment is not a label");
    let shape_tween = second.tween.as_ref().expect("a shape tween");
    assert_eq!(shape_tween.kind, TweenKind::Shape);
    let anicraft_doc::Easing::CubicBezier(x1, y1, x2, y2) = shape_tween.easing else { panic!("the custom ease's one segment: {:?}", shape_tween.easing) };
    assert!(close(x1, 0.25) && close(y1, 0.1) && close(x2, 0.75) && close(y2, 0.9), "{:?}", shape_tween.easing);
    // The instance: its symbol, matrix, loop, first frame, colour, blend and moved point.
    let e = &first.elements[0];
    let ElementKind::Symbol { symbol, loop_mode, first_frame, blend, pivot, .. } = &e.kind else { panic!("an instance: {:?}", e.kind) };
    assert_eq!(doc.symbol(*symbol).map(|s| s.name.as_str()), Some("UFO mask"));
    let want = [1.25, 0.5, -0.5, -0.5, 100.5, -20.25];
    for (got, want) in e.transform.m.iter().zip(want) {
        assert!((got - want).abs() < 1e-4, "{:?}", e.transform.m);
    }
    assert_eq!((*loop_mode, *first_frame, *blend), (LoopMode::SingleFrame, 4, BlendMode::Difference));
    let placed = e.transform.apply(pivot.expect("a moved transformation point"));
    assert!((placed.x - (inst.point.0 as f32 / 20.0)).abs() < 1e-3 && (placed.y - (inst.point.1 as f32 / 20.0)).abs() < 1e-3, "{placed:?}");
    let ColorEffect::Tint { color, amount } = e.color else { panic!("a tint: {:?}", e.color) };
    assert!((amount - 0.5).abs() < 1e-6 && color.r == 1.0 && color.g == 0.0);
    // The movie clip after the tween: placed, its extra settings skipped.
    assert!(matches!(second.elements[0].kind, ElementKind::Symbol { loop_mode: LoopMode::Loop, .. }));
    // The raw artwork of the layer in the folder: the rectangle, merging (ADR 0042).
    let raw = &tl.layers[3].keyframes[0].elements;
    assert_eq!(raw.len(), 1);
    assert!(matches!(&raw[0].kind, ElementKind::Shape(s) if s.merge));
    // The library symbol has its own timeline now, not just its first shape.
    let sym = doc.library.iter().find(|s| s.name == "UFO mask").expect("the symbol");
    let spans: Vec<(u32, u32)> = sym.timeline.layers[0].keyframes.iter().map(|k| (k.start, k.duration)).collect();
    assert_eq!(spans, [(0, 2), (2, 3)]);
    // The report: what was left out, by kind.
    assert_eq!(report.skipped.get("frame comments"), Some(&1), "{:?}", report.skipped);
    // The guide's frame script is kept as written (no publish setting: ActionScript 3.0).
    let Some(anicraft_doc::FrameScript::Source(script)) = &tl.layers[0].keyframes[0].script else { panic!("a frame script: {:?}", tl.layers[0].keyframes[0].script) };
    assert_eq!((script.text.as_str(), script.language), ("stop();", anicraft_doc::Language::ActionScript3));
    assert!(report.kept.contains_key("frame scripts in ActionScript 3.0") || report.runs.contains_key("frame scripts in ActionScript 3.0"), "{:?}", report.kept);
    assert_eq!(report.skipped.get("movie clip component settings (left out)"), Some(&1), "{:?}", report.skipped);
    assert_eq!(report.kept.get("symbol instances"), Some(&2));
}

#[test]
fn an_object_of_a_kind_not_decoded_is_skipped_and_its_siblings_still_place() {
    use anicraft_doc::ElementKind;
    let mut w = Ar::new();
    w.page(|w| {
        w.layer("notes", 0, None, |w| {
            w.frame(&[], &FrameSpec { duration: 5, ..FrameSpec::default() }, |w| {
                // Two objects of a class this pass doesn't decode (a text, in a made-up
                // layout), then an instance.
                for _ in 0..2 {
                    w.object("CPicText", 0, None, 0, 0x0D, |_| {});
                    for v in [65536, 0, 0, 65536, 40, 60] {
                        w.i32(v);
                    }
                    w.string("hello, world");
                    w.out.extend_from_slice(&[0x0C, 0x80, 0x02, 0x05, 0x41, 0x00, 0x00, 0x80]);
                }
                w.instance(&InstSpec::default());
            });
        });
    });
    let payloads = vec![("Contents", synthetic_contents()), ("P 1 1", w.out), ("Symbol 9", one_frame_stream(&rectangle_body()))];
    let (doc, report) = super::doc::import(&write(&payloads)).expect("opens");
    let k = &scene(&doc).layers[0].keyframes[0];
    assert_eq!((k.start, k.duration), (0, 5));
    assert_eq!(k.elements.len(), 1);
    assert!(matches!(k.elements[0].kind, ElementKind::Symbol { .. }));
    assert_eq!(report.skipped.get("texts whose records didn't decode"), Some(&2), "{:?}", report.skipped);
}

#[test]
fn a_timeline_cut_short_or_damaged_anywhere_never_panics() {
    let (bytes, _) = rich_page();
    let page = with_sprite_extras(bytes);
    for cut in 0..page.len() {
        let payloads = vec![("Contents", synthetic_contents()), ("P 1 1", page[..cut].to_vec())];
        let (doc, _) = super::doc::import(&write(&payloads)).expect("a damaged timeline still opens the document");
        assert_eq!(doc.scenes.len(), 1);
    }
    // Every byte flipped, one at a time.
    for at in 1..page.len() {
        let mut bad = page.clone();
        bad[at] ^= 0xA5;
        let payloads = vec![("Contents", synthetic_contents()), ("P 1 1", bad)];
        assert!(super::doc::import(&write(&payloads)).is_ok(), "flip at {at}");
    }
}

#[test]
fn the_object_graph_refuses_what_it_cannot_number() {
    use super::archive::parse;
    assert!(parse(&[]).is_err());
    assert!(parse(&[0x02]).is_err(), "the stream's first byte");
    // A back-reference to an object never written.
    assert!(parse(&[0x01, 0x05, 0x00]).is_err());
    // A class reference to a number that is no class.
    assert!(parse(&[0x01, 0x07, 0x80, 0x04, 0x00]).is_err());
    // The root must be a page.
    let mut w = Ar::new();
    w.layer("x", 0, None, |_| {});
    assert!(parse(&w.out).is_err());
}

// Texts, media, buttons, scripts, hints and folders on a synthetic document, every byte
// written here from the grammar in `crate::archive` and `crate::library` (no corpus bytes).

/// An MFC Unicode string (`FF FE FF` and a u8 length).
fn cstr(s: &str) -> Vec<u8> {
    let mut w = Ar::new();
    w.out.clear();
    w.string(s);
    w.out
}

/// A zlib stream of stored (uncompressed) deflate blocks: the header, blocks of at most
/// 65535 bytes, and the Adler-32 of the data.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(65535).collect() };
    for (i, c) in chunks.iter().enumerate() {
        out.push(u8::from(i + 1 == chunks.len()));
        out.extend((c.len() as u16).to_le_bytes());
        out.extend((!(c.len() as u16)).to_le_bytes());
        out.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    out.extend(((b << 16) | a).to_be_bytes());
    out
}

/// Animate's lossless bitmap payload: header, then the zlib stream in chunks of at most 2048
/// bytes, then a zero length. `argb` is premultiplied A, R, G, B rows.
fn bitmap_payload(w: u16, h: u16, argb: &[u8]) -> Vec<u8> {
    let mut out = vec![0x03, 0x05];
    out.extend((w * 4).to_le_bytes());
    out.extend(w.to_le_bytes());
    out.extend(h.to_le_bytes());
    for v in [0i32, i32::from(w) * 20, 0, i32::from(h) * 20] {
        out.extend(v.to_le_bytes());
    }
    out.extend([1, 1]);
    for c in zlib_stored(argb).chunks(2048) {
        out.extend((c.len() as u16).to_le_bytes());
        out.extend_from_slice(c);
    }
    out.extend([0, 0]);
    out
}

/// An item's identity block: u32 6, u32 1, its parent folder's id, u32 1, its own id.
fn identity(parent: (u32, u32), id: (u32, u32)) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [6, 1, parent.0, parent.1, 1, id.0, id.1] {
        out.extend(v.to_le_bytes());
    }
    out
}

/// Every item's common settings after its identity block.
fn common_settings() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(0u32.to_le_bytes());
    out.extend(7u32.to_le_bytes());
    out.push(0);
    for _ in 0..3 {
        out.extend(cstr(""));
    }
    out.push(0);
    out.extend(2u32.to_le_bytes());
    for _ in 0..2 {
        out.extend(cstr(""));
    }
    out.extend([0; 8]);
    out.extend(1u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(u32::MAX.to_le_bytes());
    out.push(0);
    out.extend(cstr(""));
    out.push(0);
    out.extend(1u32.to_le_bytes());
    out
}

const ART: (u32, u32) = (0x5A00_0001, 0x11);
const SOUNDS: (u32, u32) = (0x5A00_0001, 0x12);

/// `Contents` with the scene, the symbol (in "Art"), two folders ("Art" and "Art/Sounds"), a
/// bitmap (id 7, in "Art"), a sound (id 8, in "Art/Sounds") and the ActionScript 2 setting.
fn media_contents() -> Vec<u8> {
    let mut c = synthetic_contents();
    // `synthetic_contents` ends its symbol entry with 8 zero bytes: write the entry's own
    // settings there instead (a string, u32 1, the identity block).
    c.truncate(c.len() - 8);
    c.extend(cstr(""));
    c.extend(1u32.to_le_bytes());
    c.extend(identity(ART, (0x5A00_0002, 0x31)));
    c.extend(common_settings());
    // The bitmap.
    c.extend([0xFF, 0xFF, 0x01, 0x00, 0x0A, 0x00]);
    c.extend(b"CMediaBits");
    c.push(0x06);
    c.push(7);
    c.extend("Media 1".encode_utf16().flat_map(u16::to_le_bytes));
    c.extend(cstr("badge.png"));
    c.extend(7u16.to_le_bytes());
    c.extend(cstr("..\\art\\badge.png"));
    c.extend(1_300_000_000u32.to_le_bytes());
    c.extend(identity(ART, (0x5A00_0003, 0x41)));
    c.extend(common_settings());
    c.extend([0x03, 0x01, 0x32, 0x01, 0, 0, 0, 0]);
    // The sound: 44.1 kHz 16-bit mono, 4 frames.
    c.extend([0xFF, 0xFF, 0x01, 0x00, 0x0B, 0x00]);
    c.extend(b"CMediaSound");
    c.push(0x06);
    c.push(5);
    c.extend("M 2 1".encode_utf16().flat_map(u16::to_le_bytes));
    c.extend(cstr("beep"));
    c.extend(8u16.to_le_bytes());
    c.extend(cstr("..\\beep.wav"));
    c.extend(1_300_000_001u32.to_le_bytes());
    c.extend(identity(SOUNDS, (0x5A00_0004, 0x51)));
    c.extend(common_settings());
    c.extend([0x0A, 0x0E, 0x00]);
    c.extend(4u32.to_le_bytes());
    c.extend(u32::MAX.to_le_bytes());
    c.extend([0; 12]);
    // The folders: a count, then each.
    c.extend(1u32.to_le_bytes());
    c.extend(2u32.to_le_bytes());
    for (name, parent, id) in [("Art", (0, 0), ART), ("Sounds", ART, SOUNDS)] {
        c.extend(4u32.to_le_bytes());
        c.extend(cstr(name));
        c.extend(identity(parent, id));
        c.extend(common_settings());
    }
    // The publish settings.
    c.extend(cstr("Vector::ActionScriptVersion"));
    c.extend(cstr("2"));
    c
}

/// A page with a dynamic text, a bitmap instance and a named button with a handler on one
/// keyframe; a sound frame; and a shape tween with two hints.
fn media_page() -> Vec<u8> {
    use crate::text_tests::{record, units, Head, Style};
    let mut w = Ar::new();
    w.page(|w| {
        w.layer("tween", 0, None, |w| {
            let spec = FrameSpec { duration: 4, tween: 2, hints: vec![((200, 400), (-100, 60)), ((0, 0), (20, 40))], ..FrameSpec::default() };
            w.frame(&[], &spec, |_| {});
            w.frame(&[], &FrameSpec { duration: 1, ..FrameSpec::default() }, |_| {});
        });
        w.layer("sound", 0, None, |w| {
            let spec = FrameSpec { duration: 5, sound: 8, envelope: vec![(44_100, 32768, 16384)], loops: 3, sync: 3, in_out: (0, 0x3FFF_FFFF), script: "gotoAndPlay(1);\r\n", ..FrameSpec::default() };
            w.frame(&[], &spec, |_| {});
        });
        w.layer("objects", 0, None, |w| {
            w.frame(&[], &FrameSpec { duration: 5, ..FrameSpec::default() }, |w| {
                // A dynamic text field (kept with or without fonts).
                w.tag("CPicText");
                let head = Head { field: 0x0003, matrix: [65536, 0, 0, 65536, 400, 600], bounds: [0, 2000, 0, 400], ..Head::default() };
                w.out.extend(record(&head, &[(units("Score"), Style::default())]));
                // A bitmap, scaled 2× and moved (10, 20) px.
                w.object("CPicBitmap", 0, None, 0, 0x02, |_| {});
                for v in [131072, 0, 0, 131072, 200, 400] {
                    w.i32(v);
                }
                w.u16(7);
                w.out.push(0);
                // A button: its handler, track as menu off, its name, then u32 0.
                w.instance(&InstSpec { class: "CPicButton", ..InstSpec::default() });
                w.out.push(0x0B);
                w.out.extend_from_slice(&[4, 0, 0, 0, 1, 0, 0, 0, 0x10, 0x20, 0, 0, 0, 0, 0, 0]);
                w.string("on (release) {\r\n\tgetURL(\"https://example.org\", \"_self\");\r\n}");
                w.out.push(0);
                w.string("CC");
                w.i32(0);
            });
        });
    });
    w.out
}

fn media_document() -> Vec<u8> {
    // A 2 × 1 bitmap: opaque red, then half-transparent white (premultiplied).
    let bitmap = bitmap_payload(2, 1, &[255, 255, 0, 0, 128, 128, 128, 128]);
    let pcm: Vec<u8> = [0i16, 1000, -1000, 32767].iter().flat_map(|v| v.to_le_bytes()).collect();
    write(&[("Contents", media_contents()), ("P 1 1", media_page()), ("Symbol 9", one_frame_stream(&rectangle_body())), ("Media 1", bitmap), ("M 2 1", pcm)])
}

#[test]
fn texts_bitmaps_buttons_sounds_scripts_and_hints_come_in_on_their_keyframes() {
    use anicraft_doc::{ElementKind, FrameScript, Language, LibraryEntry, LibraryItem, Paint, SoundEffect, SoundSync};
    let (doc, report) = super::doc::import(&media_document()).expect("the synthetic document opens");
    let tl = scene(&doc);
    let names: Vec<&str> = tl.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["objects", "sound", "tween"]);
    // The library: the bitmap's pixels (straight alpha), the sound as a WAV, both in folders.
    assert_eq!(doc.bitmaps.len(), 1);
    let b = &doc.bitmaps[0];
    assert_eq!((b.name.as_str(), b.width, b.height), ("badge.png", 2, 1));
    let px = b.pixels().expect("the bitmap decodes");
    assert_eq!(&px.rgba[..8], &[255, 0, 0, 255, 255, 255, 255, 128]);
    assert_eq!(doc.entry_path(LibraryEntry::Item(LibraryItem::Bitmap(b.id))), "Art/badge.png");
    assert_eq!(doc.sounds.len(), 1);
    let s = &doc.sounds[0];
    assert_eq!((s.name.as_str(), s.rate, s.channels, s.frames), ("beep", 44_100, 1, 4));
    assert_eq!(doc.entry_path(LibraryEntry::Item(LibraryItem::Sound(s.id))), "Art/Sounds/beep");
    let sym = &doc.library[0];
    assert_eq!(doc.entry_path(LibraryEntry::Item(LibraryItem::Symbol(sym.id))), "Art/UFO mask");
    // The objects: the dynamic text, the bitmap's rectangle, the named button.
    let k = &tl.layers[0].keyframes[0];
    assert_eq!(k.elements.len(), 3, "{:?}", k.elements.iter().map(|e| &e.kind).collect::<Vec<_>>());
    let ElementKind::Text(block) = &k.elements[0].kind else { panic!("a text: {:?}", k.elements[0].kind) };
    assert_eq!(block.text, "Score");
    assert!(block.field.is_some(), "a dynamic field");
    // Its box sits at the record's left and top, through its matrix (20, 30) px.
    assert_eq!(k.elements[0].transform.m, [1.0, 0.0, 0.0, 1.0, 20.0, 30.0]);
    let ElementKind::Shape(rect) = &k.elements[1].kind else { panic!("the bitmap's rectangle") };
    assert!(matches!(rect.fill, Some(Paint::Bitmap { bitmap, repeat: false, .. }) if bitmap == b.id));
    assert_eq!(k.elements[1].transform.m, [2.0, 0.0, 0.0, 2.0, 10.0, 20.0]);
    let bounds = rect.path.bounds();
    assert_eq!((bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y), (0.0, 0.0, 2.0, 1.0));
    assert!(matches!(k.elements[2].kind, ElementKind::Symbol { .. }));
    assert_eq!(k.elements[2].name, "CC", "a button's name comes from its own settings");
    assert_eq!(report.skipped.get("scripts on instances (left out; frame scripts are kept)"), Some(&1), "{:?}", report.skipped);
    assert!(!report.skipped.keys().any(|k| k.starts_with("button settings")), "{:?}", report.skipped);
    // The sound frame: its sound, sync, repeats and envelope; its script, kept as written.
    let k = &tl.layers[1].keyframes[0];
    let fs = k.sound.as_ref().expect("a frame sound");
    assert_eq!((fs.sound, fs.sync, fs.repeat, fs.looping), (s.id, SoundSync::Stream, 3, false));
    let SoundEffect::Custom(points) = &fs.effect else { panic!("an envelope: {:?}", fs.effect) };
    assert_eq!(points.len(), 1);
    assert!((points[0].at - 1.0).abs() < 1e-9 && points[0].left == 1.0 && points[0].right == 0.5, "{points:?}");
    let Some(FrameScript::Source(script)) = &k.script else { panic!("a frame script: {:?}", k.script) };
    assert_eq!((script.text.as_str(), script.language), ("gotoAndPlay(1);\r\n", Language::ActionScript1or2));
    assert_eq!(report.kept.get("frame scripts in ActionScript 1.0 & 2.0"), Some(&1), "{:?}", report.kept);
    assert!(!report.skipped.contains_key("sound in and out points (played whole)"));
    // The shape tween's hints, start and end, in pixels, `a` first.
    let hints = &tl.layers[2].keyframes[0].hints;
    assert_eq!(hints.len(), 2);
    assert_eq!((hints[0].start, hints[0].end), (anicraft_geom::Vec2::new(10.0, 20.0), anicraft_geom::Vec2::new(-5.0, 3.0)));
    assert_eq!((hints[1].start, hints[1].end), (anicraft_geom::Vec2::new(0.0, 0.0), anicraft_geom::Vec2::new(1.0, 2.0)));
    // Everything decoded: the report keeps them.
    for kind in ["bitmaps", "sounds", "texts", "bitmap instances", "library folders"] {
        assert!(report.kept.contains_key(kind), "{kind}: {:?}", report.kept);
    }
    let left_out: Vec<&String> = report.skipped.keys().filter(|k| k.as_str() != "scripts on instances (left out; frame scripts are kept)").collect();
    assert!(left_out.is_empty(), "{left_out:?}");
}

#[test]
fn media_that_does_not_decode_is_reported_by_name_and_its_uses_are_counted() {
    // The bitmap's stream holds damaged pixels; the sound's is not its sample count long and
    // no sound file either.
    let mut bitmap = bitmap_payload(2, 1, &[255, 255, 0, 0, 128, 128, 128, 128]);
    let n = bitmap.len();
    bitmap[n - 4] ^= 0xFF;
    let bytes = write(&[("Contents", media_contents()), ("P 1 1", media_page()), ("Symbol 9", one_frame_stream(&rectangle_body())), ("Media 1", bitmap), ("M 2 1", vec![1, 2, 3])]);
    let (doc, report) = super::doc::import(&bytes).expect("opens");
    assert!(doc.bitmaps.is_empty() && doc.sounds.is_empty());
    let named = |k: &str| report.names.get(k).cloned().unwrap_or_default();
    assert_eq!(named("bitmaps in Animate's own format AniCraft can't decode").len(), 1, "{:?}", report.names);
    assert!(named("bitmaps in Animate's own format AniCraft can't decode")[0].starts_with("badge.png: "));
    assert!(named("sounds in formats AniCraft can't decode")[0].starts_with("beep: "), "{:?}", report.names);
    assert_eq!(report.skipped.get("instances of missing bitmaps"), Some(&1));
    assert_eq!(report.skipped.get("frame sounds whose sound isn't in the file"), Some(&1));
}

#[test]
fn a_document_with_media_cut_short_or_damaged_anywhere_never_panics() {
    let page = media_page();
    let contents = media_contents();
    let bitmap = bitmap_payload(2, 1, &[255, 255, 0, 0, 128, 128, 128, 128]);
    for cut in 0..page.len() {
        let bytes = write(&[("Contents", media_contents()), ("P 1 1", page[..cut].to_vec()), ("Media 1", bitmap.clone())]);
        assert!(super::doc::import(&bytes).is_ok(), "cut at {cut}");
    }
    for at in 1..page.len() {
        let mut bad = page.clone();
        bad[at] ^= 0xA5;
        assert!(super::doc::import(&write(&[("Contents", media_contents()), ("P 1 1", bad), ("Media 1", bitmap.clone())])).is_ok(), "flip at {at}");
    }
    // `Contents` damaged anywhere: the document opens or is refused, never a panic.
    for at in 0..contents.len() {
        let mut bad = contents.clone();
        bad[at] ^= 0x5A;
        let _ = super::doc::import(&write(&[("Contents", bad), ("P 1 1", page.clone()), ("Media 1", bitmap.clone()), ("M 2 1", vec![0; 8])]));
        let _ = super::library::media_items(&contents[..at], |_| true);
        let _ = super::library::folders(&contents[..at]);
    }
}

#[test]
fn library_folders_nest_by_their_parents_and_a_loop_stays_at_the_root() {
    use super::library::{folder_paths, folders, Folder};
    let found = folders(&media_contents());
    assert_eq!(found, vec![Folder { name: "Art".into(), parent: (0, 0), id: ART }, Folder { name: "Sounds".into(), parent: ART, id: SOUNDS }]);
    let paths = folder_paths(&found);
    assert_eq!(paths.get(&SOUNDS).map(String::as_str), Some("Art/Sounds"));
    // Two folders each other's parent: neither loops forever.
    let looped = [Folder { name: "a".into(), parent: (2, 2), id: (1, 1) }, Folder { name: "b".into(), parent: (1, 1), id: (2, 2) }];
    let paths = folder_paths(&looped);
    assert_eq!(paths.get(&(1, 1)).map(String::as_str), Some("b/a"));
    assert_eq!(super::library::script_version(&media_contents()).as_deref(), Some("2"));
}
