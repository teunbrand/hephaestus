//! A `.hep` document in, a PDF out, over C.
//!
//! This exists for one caller: the macOS Quick Look preview extension in
//! `appex/`, which is Swift and needs the document rendered into something
//! `QLPreviewReply(dataOfContentType:)` accepts. PDF is that something, and it
//! is a good answer rather than a convenient one — the `pdf` backend embeds
//! every glyph it draws, so a preview looks right on a machine that has none
//! of the plot's fonts, and Quick Look renders PDF itself at whatever size the
//! panel is.
//!
//! The whole crate is two functions and a struct. Everything interesting is in
//! `hephaestus` proper; what is here is the boundary.
//!
//! **Nothing may unwind across it.** A panic crossing an `extern "C"` frame is
//! undefined behaviour, and this code runs inside a system extension host, so
//! both entry points catch. A document that cannot be read comes back as a
//! null pointer, which the Swift side reports as a failed preview — Quick Look
//! then shows its generic icon, which is the right degradation.

use hephaestus::document::{read_document, ReadContext};
use hephaestus::geometry::Size;
use hephaestus::pdf::{encode_pdf, PdfConfig, PdfScene};

/// Points per inch, and the dpi both this and the viewer's export hand the PDF
/// backend — at 72 the size *is* the MediaBox, with no conversion.
const POINTS_PER_INCH: f64 = 72.0;

/// Page size for a document that recorded no size hint.
///
/// A preview panel is a few hundred points across, so this is about the size
/// it will be shown at rather than a print size.
const DEFAULT_SIZE: (f64, f64) = (600.0, 450.0);

/// Sizes outside this are treated as absent rather than obeyed.
///
/// A hint is whatever a writer put there, and a preview has a few hundred
/// milliseconds; solving a plot for a 40-metre page inside that budget is not
/// a thing to attempt.
const SIZE_LIMITS: (f64, f64) = (36.0, 4000.0);

/// A rendered PDF, owned by Rust until [`hephaestus_pdf_free`] takes it back.
///
/// `data` is null when rendering failed, which is the only error channel: a
/// preview extension has nothing useful to do with a message.
#[repr(C)]
pub struct HepPdf {
    /// The PDF bytes, or null on failure.
    pub data: *mut u8,
    /// How many bytes `data` holds.
    pub len: usize,
    /// Allocated capacity, so the buffer can be reclaimed exactly.
    ///
    /// Carried rather than assumed equal to `len` because reconstructing the
    /// `Vec` with the wrong capacity is undefined behaviour, and `encode_pdf`
    /// makes no promise that the two match.
    pub capacity: usize,
    /// Page width in points, which is also the PDF's MediaBox width.
    pub width_pt: f64,
    /// Page height in points.
    pub height_pt: f64,
}

impl HepPdf {
    /// The failure value.
    fn failed() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
            width_pt: 0.0,
            height_pt: 0.0,
        }
    }
}

/// Render the plot document in `bytes` to a PDF.
///
/// Returns a [`HepPdf`] whose `data` is null if `bytes` is not a document this
/// build can read. The caller owns the result and must pass it to
/// [`hephaestus_pdf_free`].
///
/// # Safety
///
/// `bytes` must point to `len` readable bytes, or be null.
#[no_mangle]
pub unsafe extern "C" fn hephaestus_pdf_from_document(bytes: *const u8, len: usize) -> HepPdf {
    if bytes.is_null() || len == 0 {
        return HepPdf::failed();
    }
    let input = std::slice::from_raw_parts(bytes, len);
    // The document is attacker-controlled — somebody was sent a file — and the
    // reader is hand-rolled. Catching here is what keeps a malformed one from
    // taking down the extension host rather than just failing to preview.
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(input)));
    match rendered {
        Ok(Some(pdf)) => pdf,
        _ => HepPdf::failed(),
    }
}

/// Reclaim a [`HepPdf`] returned by [`hephaestus_pdf_from_document`].
///
/// Passing a value whose `data` is null is a no-op, so the Swift side can free
/// unconditionally.
///
/// # Safety
///
/// `pdf` must be a value returned by [`hephaestus_pdf_from_document`] and not
/// already freed.
#[no_mangle]
pub unsafe extern "C" fn hephaestus_pdf_free(pdf: HepPdf) {
    if pdf.data.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        drop(Vec::from_raw_parts(pdf.data, pdf.len, pdf.capacity));
    });
}

/// The actual work, with no pointers in it.
fn render(input: &[u8]) -> Option<HepPdf> {
    let document = read_document(input, ReadContext::builtin()).ok()?;
    let mut composition = document.composition;

    let (width, height) = document.hints.size.filter(usable).unwrap_or(DEFAULT_SIZE);
    let size = Size::new(width, height);

    // Keyed to the palette's paper anchor, like every other consumer: the
    // background sits outside the theme, and a preview on a transparent page
    // reads as a broken one.
    let background = document
        .hints
        .background
        .unwrap_or(composition.theme_ref().palette.paper);
    let config = PdfConfig::new().background(Some(background.with_alpha(1.0)));

    let mut scene = PdfScene::with_config(size, POINTS_PER_INCH, config);
    composition.render(&mut scene, size, POINTS_PER_INCH);
    let mut bytes = encode_pdf(&scene);

    // Warnings are deliberately dropped. They are the crate's degradation
    // channel and there is nowhere in a Quick Look panel to report them; the
    // picture is still the picture.
    let data = bytes.as_mut_ptr();
    let (len, capacity) = (bytes.len(), bytes.capacity());
    std::mem::forget(bytes);

    Some(HepPdf {
        data,
        len,
        capacity,
        width_pt: width,
        height_pt: height,
    })
}

/// Whether a size hint is one worth obeying.
fn usable(size: &(f64, f64)) -> bool {
    let (width, height) = *size;
    [width, height]
        .iter()
        .all(|value| value.is_finite() && *value >= SIZE_LIMITS.0 && *value <= SIZE_LIMITS.1)
}

/// Convenience for tests and for anything Rust-side that wants the same
/// rendering without the pointer dance.
#[doc(hidden)]
pub fn pdf_bytes(input: &[u8]) -> Option<(Vec<u8>, f64, f64)> {
    let pdf = render(input)?;
    // Safety: `render` just produced these from a `Vec` it forgot.
    let bytes = unsafe { Vec::from_raw_parts(pdf.data, pdf.len, pdf.capacity) };
    Some((bytes, pdf.width_pt, pdf.height_pt))
}
