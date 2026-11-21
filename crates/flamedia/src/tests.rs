//! Synthetic payloads, written here from the grammar in the crate docs (no corpus bytes: the
//! corpus is not under this repository's licence).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use super::*;

/// A bitmap payload the way Animate lays one out: the zlib header as a chunk of its own, then
/// deflate chunks of at most `chunk` bytes, then the `00 00` terminator.
fn payload(width: u16, height: u16, row_bytes: u16, has_alpha: bool, argb: &[u8], chunk: usize) -> Vec<u8> {
    let mut out = vec![BITMAP_KIND, FORMAT_ARGB32];
    out.extend_from_slice(&row_bytes.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    for twips in [0i32, i32::from(width) * 20, 0, i32::from(height) * 20] {
        out.extend_from_slice(&twips.to_le_bytes());
    }
    out.push(u8::from(has_alpha));
    out.push(1);
    let z = miniz_oxide::deflate::compress_to_vec_zlib(argb, 6);
    let (head, body) = z.split_at(2);
    for part in std::iter::once(head).chain(body.chunks(chunk)) {
        out.extend_from_slice(&(part.len() as u16).to_le_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// A 3 × 2 image: opaque red, half-transparent premultiplied grey-blue, fully transparent;
/// then opaque white, a faint pixel, opaque black.
const ARGB: [u8; 24] = [
    255, 255, 0, 0, /**/ 128, 64, 32, 128, /**/ 0, 0, 0, 0, //
    255, 255, 255, 255, /**/ 11, 11, 5, 0, /**/ 255, 0x23, 0x1f, 0x20,
];

#[test]
fn a_bitmap_decodes_to_straight_rgba_with_its_frame() {
    let data = payload(3, 2, 12, true, &ARGB, 2048);
    assert!(is_bitmap(&data));
    let h = bitmap_header(&data).unwrap();
    assert_eq!((h.format, h.row_bytes, h.width, h.height, h.has_alpha, h.compressed), (5, 12, 3, 2, true, true));
    let b = decode_bitmap(&data).unwrap();
    assert_eq!((b.width, b.height, b.has_alpha), (3, 2, true));
    assert_eq!(b.frame, Frame { left: 0, right: 60, top: 0, bottom: 40 });
    #[rustfmt::skip]
    assert_eq!(b.rgba, [
        255, 0, 0, 255,   128, 64, 255, 128,   0, 0, 0, 0,
        255, 255, 255, 255,   255, 116, 0, 11,   0x23, 0x1f, 0x20, 255,
    ]);
}

#[test]
fn premultiplied_alpha_is_undone_with_rounding_and_clamped() {
    assert_eq!(straight(0, 0), 0);
    assert_eq!(straight(200, 255), 200);
    assert_eq!(straight(64, 128), 128);
    assert_eq!(straight(1, 3), 85);
    assert_eq!(straight(2, 3), 170);
    // A channel above its alpha (damaged data) saturates instead of wrapping.
    assert_eq!(straight(200, 100), 255);
    // Every premultiplied value of every alpha comes back within rounding of the original.
    for a in 1..=255u32 {
        for c in 0..=255u32 {
            let pre = ((c * a + 127) / 255) as u8;
            let back = u32::from(straight(pre, a as u8));
            assert!(back.abs_diff(c) <= 255 / a + 1, "c {c} a {a}: {back}");
        }
    }
}

#[test]
fn chunk_sizes_dont_matter() {
    let whole = decode_bitmap(&payload(3, 2, 12, true, &ARGB, 2048)).unwrap();
    for chunk in [1, 2, 3, 7, 65_535] {
        assert_eq!(decode_bitmap(&payload(3, 2, 12, true, &ARGB, chunk)).unwrap(), whole, "chunk {chunk}");
    }
}

#[test]
fn padded_rows_skip_their_padding_and_short_rows_are_refused() {
    let mut padded = Vec::new();
    for row in ARGB.chunks(12) {
        padded.extend_from_slice(row);
        padded.extend_from_slice(&[9, 9, 9, 9]);
    }
    let a = decode_bitmap(&payload(3, 2, 16, true, &padded, 2048)).unwrap();
    assert_eq!(a, decode_bitmap(&payload(3, 2, 12, true, &ARGB, 2048)).unwrap());
    assert_eq!(decode_bitmap(&payload(3, 2, 8, true, &ARGB[..16], 2048)), Err(DatError::BadSize));
}

#[test]
fn without_alpha_every_pixel_is_opaque() {
    let b = decode_bitmap(&payload(3, 2, 12, false, &ARGB, 2048)).unwrap();
    assert!(!b.has_alpha);
    assert_eq!(&b.rgba[4..8], &[64, 32, 128, 255]);
    assert!(b.rgba.chunks(4).all(|p| p[3] == 255));
}

#[test]
fn every_truncation_is_an_error_never_a_panic() {
    let data = payload(3, 2, 12, true, &ARGB, 5);
    for n in 0..data.len() {
        let got = decode_bitmap(&data[..n]);
        assert!(got.is_err(), "cut at {n} of {} decoded", data.len());
        let _ = bitmap_header(&data[..n]);
    }
    assert_eq!(decode_bitmap(&data[..10]), Err(DatError::Truncated));
    assert_eq!(decode_bitmap(&data[..data.len() - 1]), Err(DatError::Truncated));
}

#[test]
fn any_damaged_byte_is_handled() {
    let data = payload(3, 2, 12, true, &ARGB, 7);
    for i in 0..data.len() {
        for v in [0u8, 1, 0x7f, 0x80, 0xff] {
            let mut d = data.clone();
            d[i] = v;
            if let Ok(b) = decode_bitmap(&d) {
                assert_eq!(b.rgba.len() as u64, u64::from(b.width) * u64::from(b.height) * 4);
            }
        }
    }
}

#[test]
fn oversized_and_empty_images_are_refused_before_inflating() {
    for (w, h) in [(0, 2), (3, 0), (8193, 1), (1, 8193), (u16::MAX, u16::MAX)] {
        let data = payload(w, h, w.saturating_mul(4), true, &ARGB, 2048);
        assert_eq!(decode_bitmap(&data), Err(DatError::BadSize), "{w} × {h}");
    }
    // The largest size is accepted by the header; its tiny data then falls short, cheaply.
    let data = payload(8192, 8192, 0, true, &ARGB, 2048);
    assert_eq!(decode_bitmap(&data), Err(DatError::BadSize), "row bytes 0 is narrower than the pixels");
    let big = payload(8192, 8192, 32_768, true, &ARGB, 2048);
    assert_eq!(decode_bitmap(&big), Err(DatError::Damaged("the pixel data is shorter than the image")));
}

#[test]
fn pixel_data_past_the_image_is_refused() {
    // A zlib bomb: 1 MB of zeros claimed as a 1 × 1 image stops at 4 bytes.
    let zeros = vec![0u8; 1 << 20];
    let data = payload(1, 1, 4, true, &zeros, 2048);
    assert_eq!(decode_bitmap(&data), Err(DatError::Damaged("the pixels run past the image")));
    // Not zlib at all.
    let mut data = payload(3, 2, 12, true, &ARGB, 2048);
    data[28] ^= 0xff;
    assert!(matches!(decode_bitmap(&data), Err(DatError::Damaged(_))));
}

#[test]
fn unobserved_variants_are_refused_not_guessed() {
    let good = payload(3, 2, 12, true, &ARGB, 2048);
    let with = |at: usize, v: u8| {
        let mut d = good.clone();
        d[at] = v;
        decode_bitmap(&d)
    };
    assert_eq!(with(0, 4), Err(DatError::NotBitmap));
    assert!(matches!(with(1, 3), Err(DatError::Unsupported(s)) if s.contains("format 3")));
    assert!(matches!(with(25, 0), Err(DatError::Unsupported(s)) if s.contains("uncompressed")));
    assert!(matches!(with(24, 2), Err(DatError::Unsupported(_))));
    // PNG and JPEG data are not this format (their own readers take them).
    assert_eq!(decode_bitmap(b"\x89PNG\r\n\x1a\n0000000000000000000000"), Err(DatError::NotBitmap));
    assert_eq!(decode_bitmap(&[0xff, 0xd8, 0xff, 0xe0]), Err(DatError::NotBitmap));
    assert!(!is_bitmap(&[0xff, 0xd8, 0xff, 0xe0]) && !is_bitmap(&[3, 5]));
}

#[test]
fn xfl_sound_formats_read() {
    assert_eq!(SoundFormat::from_xfl("44kHz 16bit Stereo"), Ok(SoundFormat { rate: 44_100, channels: 2 }));
    assert_eq!(SoundFormat::from_xfl("44kHz 16bit Mono"), Ok(SoundFormat { rate: 44_100, channels: 1 }));
    assert_eq!(SoundFormat::from_xfl(" 22kHz  16bit mono "), Ok(SoundFormat { rate: 22_050, channels: 1 }));
    assert_eq!(SoundFormat::from_xfl("11kHz 16bit Stereo"), Ok(SoundFormat { rate: 11_025, channels: 2 }));
    assert_eq!(SoundFormat::from_xfl("5kHz 16bit Mono"), Ok(SoundFormat { rate: 5_512, channels: 1 }));
    assert_eq!(SoundFormat::from_xfl("44kHz 8bit Stereo"), Err(DatError::Unsupported("8-bit sound data".into())));
    for bad in ["", "44kHz", "44kHz 16bit", "96kHz 16bit Stereo", "44kHz 24bit Stereo", "44kHz 16bit Quad", "44kHz 16bit Stereo extra", "44 16bit Stereo"] {
        assert!(matches!(SoundFormat::from_xfl(bad), Err(DatError::Unsupported(_))), "{bad:?}");
    }
}

#[test]
fn legacy_sound_format_bytes_read() {
    assert_eq!(SoundFormat::from_legacy(0x0f), Ok(SoundFormat { rate: 44_100, channels: 2 }));
    assert_eq!(SoundFormat::from_legacy(0x0e), Ok(SoundFormat { rate: 44_100, channels: 1 }));
    assert_eq!(SoundFormat::from_legacy(0x0a), Ok(SoundFormat { rate: 22_050, channels: 1 }));
    assert_eq!(SoundFormat::from_legacy(0x03), Ok(SoundFormat { rate: 5_512, channels: 2 }));
    assert_eq!(SoundFormat::from_legacy(0x0d), Err(DatError::Unsupported("8-bit sound data".into())));
    assert!(matches!(SoundFormat::from_legacy(0x2f), Err(DatError::Unsupported(s)) if s.contains("codec 2")));
}

fn pcm(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

#[test]
fn a_sound_is_checked_and_becomes_a_lossless_wav() {
    let stereo = SoundFormat { rate: 44_100, channels: 2 };
    let data = pcm(&[0, 1, -1, 32_767, -32_768, 3468]);
    let s = decode_sound(&data, stereo, Some(3)).unwrap();
    assert_eq!(s.frames, 3);
    assert_eq!(s.samples().collect::<Vec<_>>(), [0, 1, -1, 32_767, -32_768, 3468]);
    let wav = s.to_wav().unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1, "PCM");
    assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2, "channels");
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44_100);
    assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16, "bits");
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 12);
    assert_eq!(&wav[44..], &data[..]);

    let mono = SoundFormat { rate: 44_100, channels: 1 };
    assert_eq!(decode_sound(&data, mono, None).unwrap().frames, 6);
}

#[test]
fn a_sound_of_the_wrong_length_or_shape_is_refused() {
    let stereo = SoundFormat { rate: 44_100, channels: 2 };
    let data = pcm(&[1, 2, 3, 4]);
    assert_eq!(decode_sound(&data, stereo, Some(3)), Err(DatError::LengthMismatch { expected: 12, found: 8 }));
    assert_eq!(decode_sound(&data[..7], stereo, None), Err(DatError::Damaged("the sound doesn't hold whole sample frames")));
    assert_eq!(decode_sound(&[], stereo, None), Err(DatError::Empty));
    assert_eq!(decode_sound(&[], stereo, Some(0)), Err(DatError::Empty));
    assert!(decode_sound(&data, SoundFormat { rate: 44_100, channels: 3 }, None).is_err());
    assert!(matches!(decode_sound(&data, stereo, Some(u64::MAX)), Err(DatError::LengthMismatch { .. })));
    for n in 0..data.len() {
        let _ = decode_sound(&data[..n], stereo, Some(2));
        let _ = decode_sound(&data[..n], stereo, None);
    }
}

#[test]
fn errors_read_as_sentences() {
    assert_eq!(DatError::Unsupported("8-bit sound data".into()).to_string(), "8-bit sound data, which AniCraft doesn't read yet");
    assert_eq!(DatError::Truncated.to_string(), "the data is cut short");
}
