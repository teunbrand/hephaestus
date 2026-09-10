//! Rendering a document to a thumbnail. Needs a working wgpu adapter.

use hephaestus_thumbnailer::{encode_png, render, MAX_EDGE};

const FIXTURE: &[u8] = include_bytes!("../../hephaestus-viewer/tests/fixtures/document.hep");

#[test]
fn neither_edge_exceeds_the_size_asked_for() {
    // The contract a file manager relies on: `%s` is a *maximum*, and the
    // aspect is the document's own — so a wide plot comes back wide rather
    // than letterboxed into a square.
    for size in [1024, 512, 256, 128, 64, 32, 16] {
        let thumbnail = render(FIXTURE, size).expect("the fixture renders");
        assert!(
            thumbnail.width <= size && thumbnail.height <= size,
            "{size}px asked for, {}x{} produced",
            thumbnail.width,
            thumbnail.height
        );
        assert!(thumbnail.width >= 1 && thumbnail.height >= 1);
        // One edge should actually reach the limit, or the thumbnail is
        // needlessly small.
        assert!(
            thumbnail.width == size || thumbnail.height == size,
            "{size}px asked for but neither edge reached it: {}x{}",
            thumbnail.width,
            thumbnail.height
        );
        assert_eq!(
            thumbnail.rgba.len(),
            (thumbnail.width as usize) * (thumbnail.height as usize) * 4
        );
    }
}

#[test]
fn the_aspect_is_the_same_at_every_size() {
    let large = render(FIXTURE, 1024).expect("renders");
    let small = render(FIXTURE, 128).expect("renders");
    let ratio = |t: &hephaestus_thumbnailer::Thumbnail| f64::from(t.width) / f64::from(t.height);
    assert!(
        (ratio(&large) - ratio(&small)).abs() < 0.02,
        "aspect drifted: {} vs {}",
        ratio(&large),
        ratio(&small)
    );
}

#[test]
fn a_thumbnail_is_opaque_and_not_blank() {
    let thumbnail = render(FIXTURE, 256).expect("renders");
    assert!(
        thumbnail
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[3] == 255),
        "a thumbnail should be fully opaque"
    );
    let first = thumbnail.rgba.as_chunks::<4>().0[0];
    assert!(
        thumbnail
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| *p != first),
        "the thumbnail is a single flat color"
    );
}

#[test]
fn it_encodes_a_png_that_declares_its_resolution() {
    let thumbnail = render(FIXTURE, 512).expect("renders");
    let png = encode_png(&thumbnail).expect("encodes");
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "not a png");
    assert!(
        png.windows(4).any(|w| w == b"pHYs"),
        "no pHYs chunk, so the file declares no resolution"
    );
    assert!(png.len() > 500, "suspiciously small: {} bytes", png.len());
}

#[test]
fn a_nonsense_size_is_refused_rather_than_attempted() {
    assert!(render(FIXTURE, 0).is_err());
    assert!(render(FIXTURE, MAX_EDGE + 1).is_err());
}

#[test]
fn anything_that_is_not_a_document_fails_cleanly() {
    for bad in [
        &b""[..],
        &b"not a plot document"[..],
        &FIXTURE[..FIXTURE.len() / 3],
    ] {
        assert!(
            render(bad, 256).is_err(),
            "{} bytes produced a thumbnail",
            bad.len()
        );
    }
}

#[test]
fn a_non_square_box_is_filled_as_far_as_the_aspect_allows() {
    // What a preview pane needs: the box is the pane, which is rarely square.
    // The plot should touch whichever edge is tighter and leave slack on the
    // other, never overflow either.
    let mut renderer = hephaestus_thumbnailer::Renderer::new().expect("an adapter");
    for (w, h) in [(800, 200), (200, 800), (1000, 400), (300, 300)] {
        let thumbnail = renderer.render_boxed(FIXTURE, w, h).expect("renders");
        assert!(
            thumbnail.width <= w && thumbnail.height <= h,
            "{w}x{h} box overflowed by {}x{}",
            thumbnail.width,
            thumbnail.height
        );
        assert!(
            thumbnail.width == w || thumbnail.height == h,
            "{w}x{h} box under-filled: {}x{}",
            thumbnail.width,
            thumbnail.height
        );
    }
}

#[test]
fn one_renderer_serves_many_sizes() {
    // The preview handler holds a renderer across resizes, because acquiring
    // an adapter is most of the cost. Re-rendering has to be repeatable.
    let mut renderer = hephaestus_thumbnailer::Renderer::new().expect("an adapter");
    let first = renderer.render(FIXTURE, 256).expect("renders");
    let second = renderer.render(FIXTURE, 256).expect("renders again");
    assert_eq!(first.rgba, second.rgba, "the same request drew differently");
    let bigger = renderer
        .render(FIXTURE, 512)
        .expect("renders at a new size");
    assert!(bigger.width > first.width);
}
