//! The frame header, and the one guard against its two readers drifting.

use hephaestus_viewer::render::frame::{
    error_frame, Encoding, FrameHeader, FrameStatus, HEADER_LEN, MAGIC, VERSION,
};

#[test]
fn a_header_survives_the_round_trip() {
    let header = FrameHeader {
        flags: Encoding::PngFast.flag(),
        status: FrameStatus::Ok,
        width: 1800,
        height: 1120,
        dpi: 192.0,
        tab: 7,
        seq: 4242,
        payload_len: 96,
    };
    let buffer = header.with_payload(&[0u8; 96]);

    assert_eq!(buffer.len(), HEADER_LEN + 96);
    let read = FrameHeader::decode(&buffer).expect("a header this build wrote");
    assert_eq!(read, header);
    assert!(read.is_png());
    assert!(!read.is_draft());
}

#[test]
fn a_fractional_dpi_survives_to_a_thousandth() {
    // 1.5 and 2 are the ratios in the world, but a fractional one is
    // reachable through display scaling, and the dpi is what theme lengths in
    // points are resolved against.
    let header = FrameHeader {
        flags: 0,
        status: FrameStatus::Ok,
        width: 10,
        height: 10,
        dpi: 96.0 * 1.325,
        tab: 1,
        seq: 1,
        payload_len: 0,
    };
    let buffer = header.with_payload(&[]);
    let read = FrameHeader::decode(&buffer).expect("a header this build wrote");
    assert!((read.dpi - header.dpi).abs() < 0.001, "got {}", read.dpi);
}

#[test]
fn the_payload_starts_four_byte_aligned() {
    // `ImageData` refuses a `Uint8ClampedArray` whose offset into the buffer
    // is not a multiple of four, and the frontend takes a view rather than a
    // copy precisely to avoid moving eight megabytes twice.
    assert_eq!(HEADER_LEN % 4, 0);
}

#[test]
fn a_reply_with_no_picture_carries_its_reason() {
    let buffer = FrameHeader::empty(FrameStatus::Superseded, 3, 9).with_payload(&[]);
    let read = FrameHeader::decode(&buffer).expect("a header this build wrote");
    assert_eq!(read.status, FrameStatus::Superseded);
    assert_eq!(read.payload_len, 0);
    assert_eq!(read.tab, 3);
    assert_eq!(read.seq, 9);

    let buffer = error_frame(3, 9, "no adapter");
    let read = FrameHeader::decode(&buffer).expect("a header this build wrote");
    assert_eq!(read.status, FrameStatus::Error);
    assert_eq!(
        std::str::from_utf8(&buffer[HEADER_LEN..]).expect("utf-8"),
        "no adapter"
    );
}

#[test]
fn anything_that_is_not_a_frame_is_refused() {
    assert!(FrameHeader::decode(&[]).is_none());
    assert!(FrameHeader::decode(&[0u8; HEADER_LEN]).is_none());

    // Right magic, wrong version: a buffer from a build that does not agree
    // with this one about the layout must not be read as pixels.
    let mut buffer = FrameHeader::empty(FrameStatus::Ok, 1, 1).with_payload(&[]);
    buffer[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());
    assert!(FrameHeader::decode(&buffer).is_none());
}

#[test]
fn the_encoding_is_overridable_by_environment() {
    // The point of the seam: raw pixels and a PNG are the same frame path, so
    // the two can be compared on one machine. Not a `#[test]` of `resolve()`
    // itself, which reads process-wide state.
    assert_eq!(Encoding::RawRgba.flag(), 0);
    assert_ne!(Encoding::PngFast.flag(), 0);
    assert_eq!(Encoding::default(), Encoding::RawRgba);
}

/// The frontend has its own parser and no compiler to catch it drifting from
/// this one, so the constants it hardcodes are checked against the source.
///
/// Crude, and the same bargain `crates/hephaestus-wasm/verify-dist.mjs`
/// makes: a static check that cannot prove the two agree, against the
/// alternative of finding out from a frame that decodes as noise.
#[test]
fn the_frontend_parser_agrees_about_the_layout() {
    let js = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ui/frame.js"))
        .expect("ui/frame.js is part of this crate");

    assert!(
        js.contains(&format!("export const HEADER_LEN = {HEADER_LEN};")),
        "ui/frame.js does not agree that the header is {HEADER_LEN} bytes"
    );

    // The magic as the frontend reads it: one little-endian u32.
    let magic = u32::from_le_bytes(*MAGIC);
    assert!(
        js.contains(&format!("{magic:#x}")),
        "ui/frame.js does not hold {magic:#x} as the frame magic"
    );

    // The single-byte fields, which take no endianness argument.
    for (name, offset) in [("flags", 6), ("status", 7)] {
        assert!(
            js.contains(&format!("getUint8({offset})")),
            "ui/frame.js reads no byte at offset {offset}, where {name} is"
        );
    }

    // The multi-byte ones, all little-endian.
    for (name, offset) in [
        ("width", 8),
        ("height", 12),
        ("dpi", 16),
        ("tab", 20),
        ("seq", 24),
        ("payloadLength", 28),
    ] {
        assert!(
            js.contains(&format!("({offset}, true)")),
            "ui/frame.js reads no little-endian field at offset {offset}, where {name} is"
        );
    }
}
