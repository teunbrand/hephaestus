//! Read a plot document and write it as SVG, with no renderer in the build.
//!
//! The native counterpart to `crates/hephaestus-svg-wasm`: the same document,
//! the same backend, the same 96 dpi, so markup produced here and markup the
//! client emits in a page agree. Two things that makes it good for:
//!
//! - **Exporting a figure** from a document without a GPU anywhere — this
//!   builds on `--no-default-features --features document-read,svg`, which
//!   is a renderer-free configuration on the oldest supported rustc.
//! - **Seeding a page.** A producer can put this markup straight in the
//!   served HTML; the client replaces it once the module boots, and because
//!   both halves come from one document at one size the swap is exact. That
//!   is the SVG client's answer to the canvas client's PNG placeholder, and
//!   it needs no second format.
//!
//! Deliberately not `document-write`: a consumer compiles only the half it
//! uses, and this example is the shape a reading build takes.
//!
//! ```sh
//! cargo run --example document_save --features document-write
//! cargo run --example document_svg --features document-read,svg
//! ```

use hephaestus::document::{read_document, ReadContext};
use hephaestus::geometry::Size;
use hephaestus::svg::{write_svg, SvgConfig, SvgScene};
use hephaestus::text::GenericFamilyKind;

/// CSS pixels per inch, and what the wasm client always draws at.
///
/// An SVG has no backing store whose resolution has to be chosen, so there is
/// no device pixel ratio here — a viewer rasterises the markup for whatever
/// display it lands on.
const CSS_DPI: f64 = 96.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/document.hep".to_string());
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            eprintln!("run: cargo run --example document_save --features document-write");
            std::process::exit(1);
        }
    };

    // Before reading, so the theme's generic family resolves to something the
    // moment anything shapes.
    register_client_fonts();

    let doc = match read_document(&bytes, ReadContext::builtin()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read the document: {e}");
            std::process::exit(1);
        }
    };
    let mut view = doc.composition;

    // The writer's size is a hint rather than a constraint — the point of a
    // document is that it re-solves at whatever size it is given — so the two
    // outputs below are the same plot laid out twice, not one scaled.
    let (w, h) = doc.hints.size.unwrap_or((900.0, 420.0));
    for (name, size) in [
        ("examples/document_svg.svg", Size::new(w, h)),
        ("examples/document_svg_narrow.svg", Size::new(w / 2.0, h)),
    ] {
        let config = SvgConfig::new()
            .background(doc.hints.background)
            // Two SVGs inlined into one page that both define `#lg0` would
            // have the second's `url(#lg0)` resolve to the first's gradient.
            .id_prefix("hep-")
            // What makes the markup hit-testable in a page: `data-pick-id` on
            // marks that carry one, and `<g data-pick-kind>` around the
            // chrome that does not.
            .pick_ids(true);

        let mut scene = SvgScene::with_config(size, CSS_DPI, config);
        view.render(&mut scene, size, CSS_DPI);

        if let Err(e) = write_svg(name, &scene) {
            eprintln!("cannot write {name}: {e}");
            std::process::exit(1);
        }
        let warnings = scene.warnings();
        if warnings.is_empty() {
            println!("{name} — {} x {}", size.width, size.height);
        } else {
            println!("{name} — {} x {}, {warnings:?}", size.width, size.height);
        }
    }
}

/// Register the faces the wasm clients ship, so markup made here and markup
/// made there shape identically.
///
/// A missing directory is a warning rather than a failure: the file is still
/// valid, it just stops being comparable to what a page would draw.
fn register_client_fonts() {
    let dir = std::path::Path::new("crates/hephaestus-wasm/fonts");
    let mut families: Vec<String> = Vec::new();
    for face in ["regular", "bold", "italic", "bolditalic"] {
        let path = dir.join(format!("roboto-{face}.ttf"));
        match std::fs::read(&path) {
            Ok(bytes) => families.extend(hephaestus::text::register_font_families(bytes)),
            Err(e) => {
                eprintln!("warning: cannot read {}: {e}", path.display());
                eprintln!("the markup will not match what the wasm client draws");
                return;
            }
        }
    }
    families.sort();
    families.dedup();
    // After the faces, so the names the mapping points at exist. Without
    // this a theme asking for `sans-serif` resolves to nothing and every
    // label is laid out as if it were empty.
    hephaestus::text::set_generic_family(GenericFamilyKind::SansSerif, &families);
    println!("registered {}", families.join(", "));
}
