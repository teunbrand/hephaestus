//! The one thing this crate does, checked without Swift in the way.

const FIXTURE: &[u8] = include_bytes!("../../hephaestus-viewer/tests/fixtures/document.hep");

#[test]
fn a_document_becomes_a_pdf_at_its_own_size() {
    let (bytes, width, height) =
        hephaestus_quicklook::pdf_bytes(FIXTURE).expect("the fixture reads");

    assert!(bytes.starts_with(b"%PDF-"), "not a pdf");
    assert!(
        bytes.len() > 1000,
        "suspiciously small: {} bytes",
        bytes.len()
    );

    // dpi 72 means the size *is* the MediaBox, with no conversion — the same
    // reasoning the viewer's vector export uses.
    let text = String::from_utf8_lossy(&bytes);
    let expected = format!("/MediaBox [0 0 {} {}]", width.round(), height.round());
    assert!(text.contains(&expected), "no {expected} in the file");

    // The fixture records a size hint, so the page should be that rather than
    // the fallback.
    assert!(width > 0.0 && height > 0.0);
}

#[test]
fn a_preview_embeds_its_fonts() {
    // The reason PDF is the right reply type: a preview has to look right on
    // a machine with none of the plot's fonts, and this backend embeds every
    // glyph it draws.
    let (bytes, _, _) = hephaestus_quicklook::pdf_bytes(FIXTURE).expect("the fixture reads");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/FontFile"), "no embedded font program");
}

#[test]
fn anything_that_is_not_a_document_fails_rather_than_panicking() {
    for bad in [
        &b""[..],
        &b"not a hephaestus document at all"[..],
        &[0xff; 64][..],
        // A real document cut in half, which is the realistic bad input.
        &FIXTURE[..FIXTURE.len() / 2],
    ] {
        assert!(
            hephaestus_quicklook::pdf_bytes(bad).is_none(),
            "{} bytes of rubbish produced a preview",
            bad.len()
        );
    }
}

#[test]
fn the_c_boundary_round_trips_and_frees() {
    // Exercised through the actual `extern "C"` entry points, since that is
    // what Swift calls and the pointer arithmetic is the part worth checking.
    let pdf = unsafe {
        hephaestus_quicklook::hephaestus_pdf_from_document(FIXTURE.as_ptr(), FIXTURE.len())
    };
    assert!(!pdf.data.is_null());
    assert!(pdf.len > 1000);
    assert!(pdf.capacity >= pdf.len);
    let head = unsafe { std::slice::from_raw_parts(pdf.data, 5) };
    assert_eq!(head, b"%PDF-");
    unsafe { hephaestus_quicklook::hephaestus_pdf_free(pdf) };

    // Freeing a failure value is a no-op, so Swift can free unconditionally.
    let empty = unsafe { hephaestus_quicklook::hephaestus_pdf_from_document(std::ptr::null(), 0) };
    assert!(empty.data.is_null());
    unsafe { hephaestus_quicklook::hephaestus_pdf_free(empty) };
}
