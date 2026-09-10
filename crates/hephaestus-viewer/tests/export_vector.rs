//! Exporting SVG and PDF, which needs no GPU.
//!
//! Worth its own test file for that reason: these run on a machine with no
//! adapter, which is the configuration the export path deliberately supports
//! — a viewer that cannot rasterize can still write a figure out.

use std::path::PathBuf;

use hephaestus_viewer::channel::TabId;
use hephaestus_viewer::render::doc::OpenDoc;
use hephaestus_viewer::render::export::{export, ExportFormat, ExportSpec, SizeUnit};

const FIXTURE: &[u8] = include_bytes!("fixtures/document.hep");

fn fixture_doc() -> OpenDoc {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/document.hep"
    ));
    OpenDoc::open(TabId(1), path, FIXTURE.to_vec(), false).expect("the fixture reads")
}

fn scratch(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "hephaestus-viewer-test-{}-{name}",
        std::process::id()
    ));
    path
}

fn spec(format: ExportFormat) -> ExportSpec {
    ExportSpec {
        format,
        width: 7.0,
        height: 5.0,
        unit: SizeUnit::In,
        dpi: 300.0,
        jpeg_quality: 90,
        outline_text: false,
        embed_fonts: false,
    }
}

#[test]
fn svg_is_written_at_the_physical_size_asked_for() {
    let doc = fixture_doc();
    let path = scratch("export.svg");
    let report = export(&doc, None, &spec(ExportFormat::Svg), &path).expect("svg export");

    let text = std::fs::read_to_string(&path).expect("the file was written");
    let _ = std::fs::remove_file(&path);

    assert!(
        text.starts_with("<?xml") || text.starts_with("<svg"),
        "{}",
        &text[..40]
    );
    // 7 × 5 inches is 504 × 360 pt, and the size is emitted in points so the
    // file has a physical size rather than only a pixel one.
    assert!(text.contains("504"), "no 504pt width in the root element");
    assert!(text.contains("360"), "no 360pt height in the root element");
    assert!(
        text.contains("pt\""),
        "the root element carries no pt units"
    );
    assert_eq!((report.width, report.height), (504, 360));
    assert!(
        report.bytes > 1000,
        "an empty-looking svg: {} bytes",
        report.bytes
    );

    // Editable output is the whole reason this backend exists, so text should
    // be text.
    assert!(text.contains("<text"), "the svg holds no <text> element");
}

#[test]
fn svg_can_be_asked_for_outlines_instead() {
    let doc = fixture_doc();
    let path = scratch("outline.svg");
    let mut spec = spec(ExportFormat::Svg);
    spec.outline_text = true;
    export(&doc, None, &spec, &path).expect("svg export");

    let text = std::fs::read_to_string(&path).expect("the file was written");
    let _ = std::fs::remove_file(&path);
    assert!(
        !text.contains("<text"),
        "outline mode should emit no <text> element"
    );
}

#[test]
fn a_pdf_page_is_the_size_that_was_asked_for() {
    let doc = fixture_doc();
    let path = scratch("export.pdf");
    let report = export(&doc, None, &spec(ExportFormat::Pdf), &path).expect("pdf export");

    let bytes = std::fs::read(&path).expect("the file was written");
    let _ = std::fs::remove_file(&path);

    assert!(bytes.starts_with(b"%PDF-"), "not a pdf");
    assert!(bytes.ends_with(b"%%EOF\n") || bytes.ends_with(b"%%EOF"));
    // A PDF's units *are* points, which is why the export path hands the
    // vector backends a dpi of 72: the size then needs no conversion to
    // become the MediaBox.
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("/MediaBox [0 0 504 360]"),
        "the page is not 504 × 360 pt"
    );
    assert_eq!((report.width, report.height), (504, 360));
    assert!(report.bytes > 1000);
}

#[test]
fn millimeters_and_points_reach_the_same_place_as_inches() {
    let doc = fixture_doc();
    for (unit, width, height) in [
        (SizeUnit::Mm, 177.8, 127.0),
        (SizeUnit::Pt, 504.0, 360.0),
        (SizeUnit::Px, 2100.0, 1500.0),
    ] {
        let path = scratch("unit.pdf");
        let mut spec = spec(ExportFormat::Pdf);
        spec.unit = unit;
        spec.width = width;
        spec.height = height;
        let report = export(&doc, None, &spec, &path).expect("pdf export");
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            (report.width, report.height),
            (504, 360),
            "{unit:?} did not resolve to the same page"
        );
    }
}

#[test]
fn a_raster_format_says_so_rather_than_writing_nothing() {
    let doc = fixture_doc();
    let path = scratch("nope.png");
    let error =
        export(&doc, None, &spec(ExportFormat::Png), &path).expect_err("no renderer was supplied");
    assert!(
        error.to_string().contains("SVG and PDF need none"),
        "unhelpful message: {error}"
    );
    assert!(!path.exists(), "a failed export should write no file");
}

#[test]
fn an_impossible_size_is_refused_before_anything_is_allocated() {
    let doc = fixture_doc();
    let path = scratch("huge.png");
    let mut spec = spec(ExportFormat::Png);
    spec.width = 100.0;
    spec.dpi = 1200.0;
    let error = export(&doc, None, &spec, &path).expect_err("120000 px is not renderable");
    // Reported as a size problem rather than as a missing renderer: the check
    // has to come first, or the message names the wrong cause.
    assert!(error.to_string().contains("16384"), "unhelpful: {error}");

    for bad in [0.0, -1.0, f64::NAN] {
        let mut spec = spec.clone();
        spec.width = bad;
        spec.dpi = 300.0;
        assert!(
            export(&doc, None, &spec, &path).is_err(),
            "{bad} was accepted"
        );
    }
    let _ = std::fs::remove_file(&path);
}
