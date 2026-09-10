//! One document, on the GPU, end to end. Needs a working wgpu adapter.
//!
//! Run with `cargo test --test render_smoke`. It is deliberately not part of
//! the default `cargo test` narrative in `CLAUDE.md` for the same reason the
//! crate's own smoke test is not: a machine with no adapter should still be
//! able to check everything else.

use std::path::PathBuf;

use hephaestus::backend::hybrid::HybridRenderer;
use hephaestus::geometry::Size;
use hephaestus::scene::SceneBuilder;
use hephaestus::Renderer;
use hephaestus_viewer::channel::TabId;
use hephaestus_viewer::render::doc::OpenDoc;
use hephaestus_viewer::render::export::{export, ExportFormat, ExportSpec, SizeUnit};

const FIXTURE: &[u8] = include_bytes!("fixtures/document.hep");

fn fixture_doc(dark: bool) -> OpenDoc {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/document.hep"
    ));
    OpenDoc::open(TabId(1), path, FIXTURE.to_vec(), dark).expect("the fixture reads")
}

#[test]
fn a_frame_comes_back_opaque_and_not_blank() {
    let mut doc = fixture_doc(false);
    let mut renderer = HybridRenderer::new().expect("a working wgpu adapter");

    let (width, height) = (800u32, 600u32);
    let size = Size::new(f64::from(width), f64::from(height));
    renderer.scene().clear();
    doc.comp.render(renderer.scene(), size, 96.0);

    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    renderer
        .render_to_buffer(width, height, doc.background(), &mut pixels)
        .expect("render to buffer");

    // Opaque throughout, which is what the frame path relies on: the webview
    // paints onto a context created without alpha, and an opaque clear is
    // also what lets the unpremultiply pass take its fast path.
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255),
        "a frame should be fully opaque"
    );

    // More than one color, or the plot did not draw.
    let first = pixels.as_chunks::<4>().0[0];
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| *pixel != first),
        "the frame is a single flat color"
    );
}

#[test]
fn inverting_the_theme_changes_the_picture() {
    let mut renderer = HybridRenderer::new().expect("a working wgpu adapter");
    let (width, height) = (400u32, 300u32);
    let size = Size::new(f64::from(width), f64::from(height));

    let mut frames = Vec::new();
    for dark in [false, true] {
        let mut doc = fixture_doc(dark);
        renderer.scene().clear();
        doc.comp.render(renderer.scene(), size, 96.0);
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        renderer
            .render_to_buffer(width, height, doc.background(), &mut pixels)
            .expect("render to buffer");
        frames.push(pixels);
    }
    assert_ne!(frames[0], frames[1], "inversion drew the same picture");
}

#[test]
fn a_draft_frame_solves_the_same_layout() {
    // The claim the draft path rests on: halving the size and the dpi together
    // leaves the point-space extent unchanged, so the layout — where the ticks
    // land, where the text breaks — is the same picture at fewer samples. If
    // that were false a draft would be a *different* plot and the swap to the
    // crisp frame would jump.
    let mut renderer = HybridRenderer::new().expect("a working wgpu adapter");

    let mut full = fixture_doc(false);
    let (width, height, dpi) = (800u32, 600u32, 192.0);
    renderer.scene().clear();
    full.comp
        .render(renderer.scene(), Size::new(800.0, 600.0), dpi);
    let mut full_pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    renderer
        .render_to_buffer(width, height, full.background(), &mut full_pixels)
        .expect("render to buffer");

    let mut draft = fixture_doc(false);
    renderer.scene().clear();
    draft
        .comp
        .render(renderer.scene(), Size::new(400.0, 300.0), dpi / 2.0);
    let mut draft_pixels = vec![0u8; 400 * 300 * 4];
    renderer
        .render_to_buffer(400, 300, draft.background(), &mut draft_pixels)
        .expect("render to buffer");

    // Compared by row extent rather than pixel for pixel: the two are the
    // same geometry at different resolutions, so what has to match is where
    // the ink is, not which samples it covered. Each frame's ink is measured
    // as a fraction of its own width.
    let full_span = ink_span(&full_pixels, width as usize, height as usize);
    let draft_span = ink_span(&draft_pixels, 400, 300);
    for (a, b) in [(full_span.0, draft_span.0), (full_span.1, draft_span.1)] {
        assert!(
            (a - b).abs() < 0.02,
            "the draft's ink sits at {b:.3} where the full frame's is at {a:.3}"
        );
    }
}

/// Leftmost and rightmost column holding ink, as a fraction of the width.
fn ink_span(pixels: &[u8], width: usize, height: usize) -> (f64, f64) {
    let background = &pixels[..4];
    let mut first = width;
    let mut last = 0usize;
    for y in 0..height {
        for x in 0..width {
            let at = (y * width + x) * 4;
            if &pixels[at..at + 4] != background {
                first = first.min(x);
                last = last.max(x);
            }
        }
    }
    assert!(first <= last, "the frame holds no ink at all");
    (first as f64 / width as f64, last as f64 / width as f64)
}

#[test]
fn every_raster_format_writes_a_recognizable_file() {
    let doc = fixture_doc(false);
    let mut renderer = HybridRenderer::new().expect("a working wgpu adapter");

    for (format, magic) in [
        (ExportFormat::Png, &b"\x89PNG"[..]),
        (ExportFormat::Jpeg, &b"\xff\xd8\xff"[..]),
        (ExportFormat::Tiff, &b"II\x2a\x00"[..]),
        (ExportFormat::Webp, &b"RIFF"[..]),
    ] {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "hephaestus-viewer-smoke-{}.{}",
            std::process::id(),
            format.extension()
        ));

        let spec = ExportSpec {
            format,
            width: 4.0,
            height: 3.0,
            unit: SizeUnit::In,
            dpi: 96.0,
            jpeg_quality: 90,
            outline_text: false,
            embed_fonts: false,
        };
        let report = export(&doc, Some(&mut renderer), &spec, &path)
            .unwrap_or_else(|e| panic!("{format:?} export: {e}"));

        let bytes = std::fs::read(&path).expect("the file was written");
        let _ = std::fs::remove_file(&path);

        assert!(
            bytes.starts_with(magic),
            "{format:?} wrote something that is not a {format:?}"
        );
        assert_eq!((report.width, report.height), (384, 288));
        assert_eq!(report.dpi, 96.0);
    }
}

#[test]
fn png_export_declares_its_resolution() {
    // A raster file that declares nothing is read as 72 dpi, so a 300 dpi
    // export would claim four times its intended physical size.
    let doc = fixture_doc(false);
    let mut renderer = HybridRenderer::new().expect("a working wgpu adapter");
    let mut path = std::env::temp_dir();
    path.push(format!("hephaestus-viewer-dpi-{}.png", std::process::id()));

    let spec = ExportSpec {
        format: ExportFormat::Png,
        width: 2.0,
        height: 1.0,
        unit: SizeUnit::In,
        dpi: 300.0,
        jpeg_quality: 90,
        outline_text: false,
        embed_fonts: false,
    };
    let report = export(&doc, Some(&mut renderer), &spec, &path).expect("png export");
    let bytes = std::fs::read(&path).expect("the file was written");
    let _ = std::fs::remove_file(&path);

    assert_eq!((report.width, report.height), (600, 300));
    let physical = bytes
        .windows(4)
        .position(|window| window == b"pHYs")
        .expect("no pHYs chunk, so the file declares no resolution");
    // 300 dpi is 11811 pixels per metre, and the unit byte says metres.
    let pixels_per_metre = u32::from_be_bytes([
        bytes[physical + 4],
        bytes[physical + 5],
        bytes[physical + 6],
        bytes[physical + 7],
    ]);
    assert!(
        (11800..=11815).contains(&pixels_per_metre),
        "pHYs says {pixels_per_metre} px/m, which is not 300 dpi"
    );
}
