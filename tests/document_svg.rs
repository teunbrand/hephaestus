//! A plot document in, SVG out — the pipeline `crates/hephaestus-svg-wasm`
//! is, with no browser around it.
//!
//! The client is a `cdylib` with no Rust tests of its own, so this is where
//! the path it depends on is pinned: read a document, emit at a size, and
//! get markup that is well formed, reflowed rather than scaled, and
//! hit-testable. Each of those is something a page would otherwise discover
//! as a plot that renders wrong.
//!
//! `document` rather than `document-read`: the fixture has to be written
//! before it can be read. The client itself compiles only the read half.

#![cfg(all(feature = "document", feature = "svg"))]

use hephaestus::color::{rgb8, Color};
use hephaestus::composition::{grid, Composition, Patch};
use hephaestus::document::{read_document, write_composition, ReadContext, WriteOptions};
use hephaestus::geometry::Size;
use hephaestus::plot::chrome::axis::{Axis, AxisPlacement};
use hephaestus::plot::{scale, LineGeom, Plot, PlotComposition, PointGeom};
use hephaestus::scales::chrome::AxisSide;
use hephaestus::svg::{encode_svg, SvgConfig, SvgScene};

/// What the SVG client always draws at: CSS pixels, and no device pixel
/// ratio, because there is no backing store to size.
const CSS_DPI: f64 = 96.0;

/// The layout: one patch, which is all this needs. Nesting is
/// `document_roundtrip`'s job.
fn one_patch() -> Composition {
    grid(1, 1, vec![Patch::new("main").into()])
}

/// A composition with enough chrome to exercise scopes, and enough marks to
/// exercise ids.
fn build() -> PlotComposition {
    let x: Vec<f64> = (0..40).map(|i| f64::from(i) * 0.25).collect();
    let y: Vec<f64> = x.iter().map(|v| v.sin() * 8.0 + 12.0).collect();
    // Ids the DOM can report back. A geom emits them only when it is given a
    // `pick_id` channel — without one every mark is `Skip` and the markup
    // carries no id at all.
    let ids: Vec<f64> = (0..40).map(f64::from).collect();

    let mut plot = Plot::new(&one_patch(), "main")
        .bind("x", "t")
        .bind("y", "value")
        .title("A document, drawn as markup");
    plot.add_geom(
        LineGeom::builder()
            .set("x", x.clone())
            .set("y", y.clone())
            // `LineDefaults::stroke` is `None` by design, so a line with no
            // stroke channel draws nothing.
            .set("stroke", rgb8(60, 90, 200))
            .build(),
    );
    plot.add_geom(
        PointGeom::builder()
            .set("x", x)
            .set("y", y)
            .set("pick_id", ids)
            .set("fill", rgb8(200, 60, 60))
            .build(),
    );
    plot.add_axis(Axis::rail("t", AxisPlacement::Cartesian(AxisSide::Bottom)).title("t"));
    plot.add_axis(Axis::rail("value", AxisPlacement::Cartesian(AxisSide::Left)).title("value"));

    PlotComposition::new(&one_patch())
        .with_plot(plot)
        .add_scale("t", scale::continuous(0.0..=10.0))
        .add_scale("value", scale::continuous(0.0..=22.0))
}

/// Write the fixture and read it back, the way a page would meet it.
fn roundtrip() -> PlotComposition {
    let bytes = write_composition(&build(), &WriteOptions::default()).expect("writes");
    read_document(&bytes, ReadContext::builtin())
        .expect("reads")
        .composition
}

/// Emit at a size, with the client's own options.
fn emit(view: &mut PlotComposition, width: f64, height: f64) -> (String, Vec<String>) {
    let size = Size::new(width, height);
    let config = SvgConfig::new()
        .background(Some(Color::WHITE))
        .id_prefix("t-")
        .pick_ids(true);
    let mut scene = SvgScene::with_config(size, CSS_DPI, config);
    view.render(&mut scene, size, CSS_DPI);
    let warnings = scene.warnings().iter().map(|w| format!("{w:?}")).collect();
    (encode_svg(&scene), warnings)
}

#[test]
fn a_document_reloaded_emits_a_well_formed_document() {
    let mut view = roundtrip();
    let (svg, warnings) = emit(&mut view, 900.0, 420.0);

    assert!(svg.starts_with("<svg"), "{}", &svg[..80.min(svg.len())]);
    assert!(svg.ends_with("</svg>"));
    assert_eq!(
        svg.matches("<g ").count(),
        svg.matches("</g>").count(),
        "every group is closed"
    );
    // The size asked for reaches the root, so a page's box is what the
    // markup claims rather than whatever the writer happened to use.
    assert!(svg.contains(r#"width="900""#), "{svg}");
    assert!(svg.contains(r#"height="420""#), "{svg}");
    // Nothing in this plot is outside what SVG expresses; a warning here
    // would mean the client is showing an approximation.
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
}

#[test]
fn text_survives_the_round_trip_as_text() {
    let mut view = roundtrip();
    let (svg, _) = emit(&mut view, 900.0, 420.0);

    // The whole point of the backend, and the half that silently vanishes
    // when no font is registered: a document lays out as if every string
    // were empty and the plot arrives as chrome with no labels.
    assert!(svg.contains("<text"), "no text elements: {svg}");
    assert!(
        svg.contains("A document, drawn as markup"),
        "the title did not survive as source text"
    );
}

#[test]
fn a_new_size_reflows_rather_than_scaling() {
    let mut view = roundtrip();
    let (narrow, _) = emit(&mut view, 420.0, 400.0);
    let (wide, _) = emit(&mut view, 1400.0, 400.0);

    assert_ne!(
        narrow, wide,
        "the same markup came back at a different size"
    );

    // Scaling would leave the interior coordinates alone and change only the
    // root's width, so the distinguishing evidence is that content *moved*.
    // Compare the two with their roots removed.
    let body = |s: &str| s[s.find('>').expect("a root") + 1..].to_string();
    assert_ne!(
        body(&narrow),
        body(&wide),
        "only the root element differed — the layout was scaled, not re-solved"
    );
}

#[test]
fn the_markup_is_hit_testable_for_marks_and_for_chrome() {
    let mut view = roundtrip();
    let (svg, _) = emit(&mut view, 900.0, 420.0);

    // A geom given a `pick_id` channel reports ids a page can read back.
    assert!(svg.contains("data-pick-id="), "no pick ids: {svg}");
    // Chrome carries no id of its own, so it is identified by the scope
    // groups around it — which is what lets a hover report "the bottom
    // axis" rather than only "row 12".
    assert!(svg.contains("data-pick-kind="), "no pick scopes: {svg}");
    assert!(
        svg.contains(r#"data-pick-name="axis_tick_label""#),
        "tick labels are not scoped: {svg}"
    );

    // And chrome stays hittable. A `Skip` primitive inside a `Target` scope
    // *is* the target, so writing `pointer-events="none"` on it would leave
    // every label unreachable under `elementFromPoint` while the CPU index
    // answered for the same scene.
    let labels = svg
        .find(r#"data-pick-name="axis_tick_label""#)
        .expect("a tick label");
    let group_end = svg[labels..].find("</g>").expect("a closed group") + labels;
    assert!(
        !svg[labels..group_end].contains(r#"pointer-events="none""#),
        "a tick label opted out of hit testing: {}",
        &svg[labels..group_end]
    );
}

#[test]
fn picking_attributes_are_one_flag() {
    let mut view = roundtrip();
    let size = Size::new(900.0, 420.0);
    let mut scene = SvgScene::with_config(size, CSS_DPI, SvgConfig::new());
    view.render(&mut scene, size, CSS_DPI);
    let svg = encode_svg(&scene);

    // Off is the right default for file export, and a dense plot that is
    // never hit-tested should not pay for the attributes.
    assert!(!svg.contains("data-pick-id"), "{svg}");
    assert!(!svg.contains("data-pick-kind"), "{svg}");
    assert!(!svg.contains("pointer-events"), "{svg}");
}

#[test]
fn an_id_prefix_keeps_two_documents_on_one_page_apart() {
    let mut view = roundtrip();
    let size = Size::new(900.0, 420.0);

    let mut a = SvgScene::with_config(size, CSS_DPI, SvgConfig::new().id_prefix("a-"));
    view.render(&mut a, size, CSS_DPI);
    let mut b = SvgScene::with_config(size, CSS_DPI, SvgConfig::new().id_prefix("b-"));
    view.render(&mut b, size, CSS_DPI);

    // Whatever ids this plot allocates, the two documents must not share
    // one: inlined together, the second's `url(#…)` would resolve to the
    // first's definition in every browser.
    let ids = |s: &str| -> Vec<String> {
        s.match_indices(" id=\"")
            .map(|(i, m)| {
                let rest = &s[i + m.len()..];
                rest[..rest.find('"').expect("a closing quote")].to_string()
            })
            .collect()
    };
    for id in ids(&encode_svg(&a)) {
        assert!(id.starts_with("a-"), "unprefixed id {id:?}");
    }
    for id in ids(&encode_svg(&b)) {
        assert!(id.starts_with("b-"), "unprefixed id {id:?}");
    }
}
