//! The frame wire format: a fixed header the frontend parses, followed by
//! either raw pixels or an encoded PNG.
//!
//! A frame goes to the webview as one opaque byte buffer rather than as JSON,
//! because a JSON array of eight million numbers is not a thing to send. The
//! header carries everything the frontend has to know before it can look at
//! the payload — how large the picture is, what encoded it, which tab and
//! which request it answers — and it carries the *outcome* too. A request that
//! was overtaken by a newer one for the same tab comes back with
//! [`FrameStatus::Superseded`] and no payload, which the frontend drops
//! silently. Reporting that as a rejected promise would make an ordinary,
//! expected event look like a failure at every layer it passed through.

/// Bytes at the front of every frame reply.
///
/// A multiple of four so a `Uint8ClampedArray` view can start at the payload:
/// `ImageData` refuses an unaligned offset.
pub const HEADER_LEN: usize = 32;

/// Identifies a frame buffer, so a truncated or mis-routed reply is caught
/// before it is treated as pixels.
pub const MAGIC: &[u8; 4] = b"HEPF";

/// Wire version of the header. Bumped when the layout below changes.
pub const VERSION: u16 = 1;

/// Set when the payload is a PNG rather than raw RGBA8.
pub const FLAG_PNG: u8 = 1 << 0;

/// Set when the frame was rendered at half resolution.
pub const FLAG_DRAFT: u8 = 1 << 1;

/// What became of the frame request.
///
/// Rides in the header rather than in the command's `Result` so that the
/// ordinary outcomes — a frame, or a frame that stopped being wanted — travel
/// the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameStatus {
    /// The payload is a picture.
    Ok = 0,
    /// A newer request for the same tab arrived first. No payload.
    Superseded = 1,
    /// Rendering failed. The payload is the UTF-8 message.
    Error = 2,
    /// The tab was closed between the request and its turn. No payload.
    NoSuchTab = 3,
}

impl FrameStatus {
    /// Read a status back off the wire.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Ok),
            1 => Some(Self::Superseded),
            2 => Some(Self::Error),
            3 => Some(Self::NoSuchTab),
            _ => None,
        }
    }
}

/// How the pixels are carried to the webview.
///
/// The seam exists because Tauri's binary IPC is not equally fast everywhere:
/// a raw buffer measures a few milliseconds per ten megabytes on macOS and
/// far more than that on Windows. A plot is flat color and compresses hard, so
/// where the transport is the bottleneck it is cheaper to spend a fast deflate
/// than to send the bytes — and the frontend decodes a PNG off its main thread
/// through `createImageBitmap`, which raw pixels cannot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// RGBA8 exactly as the renderer produced it. No encode, no decode, and
    /// `putImageData` adopts the buffer without copying it.
    #[default]
    RawRgba,
    /// PNG at the cheap end of the filter/deflate range. Roughly a tenth the
    /// bytes on plot content for a small fraction of what a balanced encode
    /// would cost.
    PngFast,
}

impl Encoding {
    /// The encoding to use unless something says otherwise.
    ///
    /// Windows is the one platform where the transport is slow enough that
    /// encoding wins; elsewhere the raw path is both faster and simpler.
    /// `HEPHAESTUS_VIEWER_ENCODING=raw|png` overrides it, which is what makes
    /// the two comparable on one machine.
    pub fn resolve() -> Self {
        match std::env::var("HEPHAESTUS_VIEWER_ENCODING")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("raw") | Some("rgba") => Self::RawRgba,
            Some("png") => Self::PngFast,
            _ if cfg!(target_os = "windows") => Self::PngFast,
            _ => Self::RawRgba,
        }
    }

    /// Flag bit this encoding sets in a frame header.
    pub fn flag(self) -> u8 {
        match self {
            Self::RawRgba => 0,
            Self::PngFast => FLAG_PNG,
        }
    }
}

/// The fixed part of a frame reply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameHeader {
    /// `FLAG_PNG` / `FLAG_DRAFT`.
    pub flags: u8,
    /// What became of the request.
    pub status: FrameStatus,
    /// Picture width in device pixels — half the requested width on a draft
    /// frame, so the frontend sizes its backing store from this rather than
    /// from what it asked for.
    pub width: u32,
    /// Picture height in device pixels.
    pub height: u32,
    /// Dots per inch the layout was solved against. Carried to a thousandth,
    /// which is finer than any device pixel ratio in use.
    pub dpi: f64,
    /// Which document the picture belongs to.
    pub tab: u32,
    /// The frontend's request counter, echoed back so a late reply can be
    /// recognized as late.
    pub seq: u32,
    /// Bytes following the header.
    pub payload_len: u32,
}

impl FrameHeader {
    /// A header for a reply that carries no picture.
    pub fn empty(status: FrameStatus, tab: u32, seq: u32) -> Self {
        Self {
            flags: 0,
            status,
            width: 0,
            height: 0,
            dpi: 0.0,
            tab,
            seq,
            payload_len: 0,
        }
    }

    /// Write the header into the first [`HEADER_LEN`] bytes of `out`.
    ///
    /// Panics if `out` is shorter than that, which is a bug in the caller
    /// rather than a runtime condition: every buffer here is allocated with
    /// the header's room in it.
    pub fn write_into(&self, out: &mut [u8]) {
        let out = &mut out[..HEADER_LEN];
        out[0..4].copy_from_slice(MAGIC);
        out[4..6].copy_from_slice(&VERSION.to_le_bytes());
        out[6] = self.flags;
        out[7] = self.status as u8;
        out[8..12].copy_from_slice(&self.width.to_le_bytes());
        out[12..16].copy_from_slice(&self.height.to_le_bytes());
        out[16..20].copy_from_slice(&dpi_to_milli(self.dpi).to_le_bytes());
        out[20..24].copy_from_slice(&self.tab.to_le_bytes());
        out[24..28].copy_from_slice(&self.seq.to_le_bytes());
        out[28..32].copy_from_slice(&self.payload_len.to_le_bytes());
    }

    /// A header plus its payload as one buffer.
    pub fn with_payload(mut self, payload: &[u8]) -> Vec<u8> {
        self.payload_len = payload.len() as u32;
        let mut out = vec![0u8; HEADER_LEN + payload.len()];
        self.write_into(&mut out);
        out[HEADER_LEN..].copy_from_slice(payload);
        out
    }

    /// Read a header back, or `None` if `bytes` is too short or not a frame.
    ///
    /// The frontend has its own parser in `ui/frame.js`; this one is what lets
    /// a test hold the two to the same layout.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_LEN || &bytes[0..4] != MAGIC {
            return None;
        }
        if u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION {
            return None;
        }
        Some(Self {
            flags: bytes[6],
            status: FrameStatus::from_u8(bytes[7])?,
            width: read_u32(bytes, 8),
            height: read_u32(bytes, 12),
            dpi: f64::from(read_u32(bytes, 16)) / 1000.0,
            tab: read_u32(bytes, 20),
            seq: read_u32(bytes, 24),
            payload_len: read_u32(bytes, 28),
        })
    }

    /// True when the payload is a PNG.
    pub fn is_png(&self) -> bool {
        self.flags & FLAG_PNG != 0
    }

    /// True when the picture is half-resolution.
    pub fn is_draft(&self) -> bool {
        self.flags & FLAG_DRAFT != 0
    }
}

/// One frame reply, ready to hand to the webview.
pub fn error_frame(tab: u32, seq: u32, message: &str) -> Vec<u8> {
    FrameHeader::empty(FrameStatus::Error, tab, seq).with_payload(message.as_bytes())
}

/// The reply for a window that is showing no document.
///
/// Not an error: a window whose document was closed can have a frame request
/// already in flight, and an empty window asking for one is the ordinary state
/// of a freshly opened window.
pub fn no_document(seq: u32) -> Vec<u8> {
    FrameHeader::empty(FrameStatus::NoSuchTab, 0, seq).with_payload(&[])
}

/// dpi in thousandths, saturating rather than wrapping on a nonsense value.
fn dpi_to_milli(dpi: f64) -> u32 {
    let milli = (dpi * 1000.0).round();
    if milli.is_finite() && milli > 0.0 {
        milli.min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
