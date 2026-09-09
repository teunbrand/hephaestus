//! Render a hephaestus plot document to SVG in the browser.
//!
//! A document carries a plot's *configuration*, not a picture of it, so the
//! page re-solves the layout at whatever size it has. Resizing reflows —
//! axes re-lay-out, ticks recompute, text re-wraps — instead of stretching.
//!
//! The sibling of `hephaestus-wasm`, and the difference is the whole design:
//! that client rasterises onto a `<canvas>`, this one emits markup and hands
//! it back as a string. There is no GPU here, no adapter to acquire and no
//! drawing buffer to clear, so a resize is "emit new markup, replace the
//! old". What it costs is one DOM element per mark, which is why both
//! clients exist — a dense scatter still belongs on a canvas.
//!
//! This crate is the binding layer, deliberately thin. It owns the document
//! and the composition and exposes one drawing call,
//! [`PlotDocument::to_svg`]. Everything browser-shaped — `ResizeObserver`,
//! `matchMedia`, `requestAnimationFrame`, `fetch`, and hit testing, which
//! here is ordinary DOM work — lives in `js/hephaestus-svg.js`, which wraps
//! this in the `PlotView` class a page actually uses. JavaScript is free;
//! wasm bytes are not.
//!
//! ```js
//! import init, { PlotView } from './hephaestus-svg.js';
//!
//! await init();
//! const view = await PlotView.create(container, docBytes, { colorScheme: 'auto' });
//! ```

use wasm_bindgen::prelude::*;

use hephaestus::document::{read_document, DocumentHints, ReadContext};
use hephaestus::geometry::Size;
use hephaestus::plot::theme::Theme;
use hephaestus::plot::PlotComposition;
use hephaestus::svg::{encode_svg, SvgConfig, SvgScene};
use hephaestus::text::GenericFamilyKind;

/// CSS pixels per inch.
///
/// Fixed, unlike the canvas client's `96 * devicePixelRatio`. An SVG has no
/// backing store whose resolution has to be chosen — the browser rasterises
/// it for whatever display it lands on — and CSS pixels are the unit the
/// caller measured its box in, so there is nothing for a device pixel ratio
/// to do here.
const CSS_DPI: f64 = 96.0;

/// Major version of the plot-document format this build reads.
///
/// A document whose major differs is refused outright — the check is equality,
/// not a floor — so a site and whatever writes its documents have to agree on
/// this number. Publish it alongside the bundle and assert on it in a build
/// step: nothing at runtime can recover from a mismatch, and the failure is a
/// plot that never appears.
#[wasm_bindgen(js_name = documentFormatVersion)]
pub fn document_format_version() -> u16 {
    hephaestus::document::FORMAT_VERSION_MAJOR
}

/// Register every font face in `bytes`, returning the family names they
/// landed under.
///
/// A browser starts with no fonts at all — nothing enumerates a system font
/// set — so a page that registers none renders chrome with no text. Call
/// this before creating any view: the first thing a document decodes is its
/// theme, and that is enough to shape.
///
/// Shaping is needed here even though the browser does the final drawing.
/// The markup names a family and lets the browser resolve it, but the
/// advances that place each run — and the tick-label widths the layout solver
/// sizes tracks from — come from shaping in wasm. A build with no fonts lays
/// out as if every string were empty.
///
/// Registration is process-global and permanent, so one call serves every
/// view on the page for the lifetime of the module.
///
/// The family names are the point of the return value: [`set_generic_family`]
/// takes names, and the only place a family's name exists is inside the file,
/// so a caller cannot pair the two without being told. Guessing from a
/// filename does not survive contact with a real font.
///
/// Accepts TTF, OTF, TTC, OTC and — with the `webfonts` feature, on by
/// default — WOFF and WOFF2, which are unwrapped to the sfnt inside first.
/// A blob holding no recognisable face is an error rather than an empty
/// list, since registering nothing silently would render a textless plot
/// with no indication why.
#[wasm_bindgen(js_name = registerFont)]
pub fn register_font(bytes: Vec<u8>) -> Result<Vec<String>, JsError> {
    let owned = decode_webfont(bytes)?;
    let families = hephaestus::text::register_font_families(owned);
    if families.is_empty() {
        return Err(JsError::new(
            "no font faces found; the bytes are not a TTF, OTF, TTC or OTC file",
        ));
    }
    Ok(families)
}

/// Whether any font family is available to shape with.
///
/// A browser starts with none, so this answers "does this page still need a
/// font?" — and it answers it after a document has been read, so a document
/// carrying embedded faces counts. That is what lets a fallback be fetched
/// only when it is genuinely needed rather than on a guess.
#[wasm_bindgen(js_name = hasFonts)]
pub fn has_fonts() -> bool {
    !hephaestus::text::registered_families().is_empty()
}

/// Unwrap a WOFF / WOFF2 container to the sfnt inside, or pass bytes through.
///
/// The shaper ingests sfnt only (TTF / OTF / TTC / OTC), and a font CDN serves
/// a browser WOFF2 — so without this the single most likely input is the one
/// that fails.
///
/// Takes the buffer by value so an sfnt — the common case, and every face the
/// client bundles — moves straight through to the shaper. wasm-bindgen has
/// already copied the `Uint8Array` into linear memory to hand us this, and
/// `register_font_families` wants it owned, so a borrow here would mean
/// copying half a megabyte of boot-path font twice over.
#[cfg(feature = "webfonts")]
fn decode_webfont(bytes: Vec<u8>) -> Result<Vec<u8>, JsError> {
    match bytes.get(..4) {
        Some(b"wOF2") => wuff::decompress_woff2(&bytes)
            .map_err(|e| JsError::new(&format!("could not decode the WOFF2 font: {e:?}"))),
        Some(b"wOFF") => wuff::decompress_woff1(&bytes)
            .map_err(|e| JsError::new(&format!("could not decode the WOFF font: {e:?}"))),
        _ => Ok(bytes),
    }
}

/// Without the `webfonts` feature the containers are refused by name, rather
/// than reaching the shaper and registering nothing.
#[cfg(not(feature = "webfonts"))]
fn decode_webfont(bytes: Vec<u8>) -> Result<Vec<u8>, JsError> {
    match bytes.get(..4) {
        Some(b"wOF2") | Some(b"wOFF") => Err(JsError::new(
            "this build cannot decode WOFF or WOFF2; use TTF, OTF, TTC or OTC, \
             or rebuild with the `webfonts` feature",
        )),
        _ => Ok(bytes),
    }
}

/// Point a generic family at concrete families already registered.
///
/// `kind` is one of `serif`, `sans-serif`, `monospace`, `cursive`,
/// `fantasy`, `system-ui`. A generic is an indirection through the font
/// context rather than a name, so registering a font is not enough on its
/// own — a theme asking for `sans-serif` resolves to nothing until this
/// says what `sans-serif` means here. Call it after [`register_font`], since
/// names that aren't registered are skipped.
#[wasm_bindgen(js_name = setGenericFamily)]
pub fn set_generic_family(kind: &str, families: Vec<String>) -> Result<(), JsError> {
    let kind = match kind {
        "serif" => GenericFamilyKind::Serif,
        "sans-serif" => GenericFamilyKind::SansSerif,
        "monospace" | "mono" => GenericFamilyKind::Mono,
        "cursive" => GenericFamilyKind::Cursive,
        "fantasy" => GenericFamilyKind::Fantasy,
        "system-ui" => GenericFamilyKind::SystemUi,
        other => {
            return Err(JsError::new(&format!(
                "unknown generic family {other:?}; expected one of serif, \
                 sans-serif, monospace, cursive, fantasy, system-ui"
            )))
        }
    };
    hephaestus::text::set_generic_family(kind, &families);
    Ok(())
}

/// One emitted document, and whatever the backend could not express.
///
/// Warnings travel beside the markup rather than being logged, because each
/// one names something the page is showing an approximation of — a flattened
/// sweep gradient, an image this build has no codec for — and only the host
/// knows whether that is worth surfacing.
#[wasm_bindgen]
pub struct SvgRender {
    svg: String,
    warnings: Vec<String>,
}

#[wasm_bindgen]
impl SvgRender {
    /// The document, as markup ready to place in the page.
    #[wasm_bindgen(getter)]
    pub fn svg(&self) -> String {
        self.svg.clone()
    }

    /// Everything the scene expressed that SVG could not, deduplicated.
    #[wasm_bindgen(getter)]
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}

/// A plot document, ready to draw at any size.
///
/// The low-level binding. A page normally uses the `PlotView` class in
/// `js/hephaestus-svg.js`, which owns the resize and color-scheme wiring and
/// calls through to this.
///
/// The composition is kept live between draws deliberately: a resize re-solves
/// the layout, and re-reading the document to do that would put a full decode
/// behind every frame of a window drag.
#[wasm_bindgen]
pub struct PlotDocument {
    view: PlotComposition,
    /// The theme exactly as the document carried it. Never mutated, so
    /// deriving from it makes toggling light and dark idempotent — the
    /// alternative, inverting the live theme in place, drifts if a caller
    /// sets the same mode twice.
    base: Theme,
    dark: bool,
    transparent: bool,
    pick_ids: bool,
    hints: DocumentHints,
}

#[wasm_bindgen]
impl PlotDocument {
    /// Read `doc`.
    ///
    /// Synchronous, unlike the canvas client's `create`: there is no adapter
    /// or device to acquire. Fails only if the document is unreadable — a
    /// truncated file, or one written at a different format major.
    #[wasm_bindgen(js_name = load)]
    pub fn load(doc: Vec<u8>) -> Result<PlotDocument, JsError> {
        #[cfg(feature = "debug-panics")]
        console_error_panic_hook::set_once();

        // One pass, and the shared context: `read_hints` beside
        // `read_composition` decodes the head twice, and `ReadContext::new`
        // builds a geom factory table per view.
        let doc = read_document(&doc, ReadContext::builtin()).map_err(to_js)?;
        let base = doc.composition.theme_ref().clone();

        let mut out = PlotDocument {
            view: doc.composition,
            base,
            dark: false,
            transparent: false,
            pick_ids: true,
            hints: doc.hints,
        };
        out.apply_theme();
        Ok(out)
    }

    /// Draw at `width` x `height` CSS pixels and return the markup.
    ///
    /// `id_prefix` prefixes every generated id. It is not a nicety: two SVGs
    /// inlined into one page that both define `#lg0` will have the second's
    /// `url(#lg0)` resolve to the first's definition, in every browser. Pass
    /// something unique per view.
    #[wasm_bindgen(js_name = toSvg)]
    pub fn to_svg(&mut self, width: f64, height: f64, id_prefix: &str) -> SvgRender {
        let size = Size::new(width.max(1.0), height.max(1.0));
        let background = if self.transparent {
            None
        } else {
            Some(self.view.theme_ref().palette.paper)
        };
        let config = SvgConfig::new()
            .background(background)
            .id_prefix(id_prefix)
            .pick_ids(self.pick_ids);

        let mut scene = SvgScene::with_config(size, CSS_DPI, config);
        self.view.render(&mut scene, size, CSS_DPI);
        SvgRender {
            svg: encode_svg(&scene),
            // `SvgWarning` carries no `Display`, and the crate's own
            // examples print these with `{:?}`; the variant name is also the
            // more useful half for a caller that wants to switch on one.
            warnings: scene.warnings().iter().map(|w| format!("{w:?}")).collect(),
        }
    }

    /// Switch between the document's theme and its inverted form.
    ///
    /// Inversion swaps the palette's paper and ink anchors, which every
    /// chrome element references, so gridlines, axis text and titles all
    /// follow. A geom given an explicit color keeps it — marks adapt only
    /// when the plot expressed them as palette references.
    ///
    /// Does not draw; call [`Self::to_svg`] after.
    #[wasm_bindgen(js_name = setDark)]
    pub fn set_dark(&mut self, dark: bool) {
        if self.dark == dark {
            return;
        }
        self.dark = dark;
        self.apply_theme();
    }

    /// Whether the inverted theme is in use.
    #[wasm_bindgen(js_name = isDark)]
    pub fn is_dark(&self) -> bool {
        self.dark
    }

    /// Emit no background rect, so whatever is behind the SVG shows through.
    ///
    /// Off by default, which matches the canvas client: a document names a
    /// paper color and honouring it is what makes the two clients agree. A
    /// page that owns its own light and dark wants this on.
    #[wasm_bindgen(js_name = setTransparent)]
    pub fn set_transparent(&mut self, transparent: bool) {
        self.transparent = transparent;
    }

    /// Whether the background rect is suppressed.
    #[wasm_bindgen(js_name = isTransparent)]
    pub fn is_transparent(&self) -> bool {
        self.transparent
    }

    /// Emit `data-pick-id` on primitives and `data-pick-kind` on scope groups.
    ///
    /// On by default, because hit testing an inline SVG is what the attributes
    /// are for and a page gets it with no wasm involved. Turn it off for a
    /// dense plot that is never hit-tested: the attributes are pure weight
    /// there, and `pointer-events="none"` lands on every unpicked primitive.
    ///
    /// Does not draw; call [`Self::to_svg`] after.
    #[wasm_bindgen(js_name = setPickIds)]
    pub fn set_pick_ids(&mut self, on: bool) {
        self.pick_ids = on;
    }

    /// Whether picking attributes are emitted.
    #[wasm_bindgen(js_name = pickIds)]
    pub fn pick_ids(&self) -> bool {
        self.pick_ids
    }

    /// Width, in points, the document's writer rendered at, if it recorded one.
    ///
    /// Advisory — a document can be drawn at any size. Useful as an aspect
    /// ratio for a container that has to be sized before anything is laid out.
    #[wasm_bindgen(js_name = hintWidth)]
    pub fn hint_width(&self) -> Option<f64> {
        self.hints.size.map(|(w, _)| w)
    }

    /// Height, in points, the document's writer rendered at, if it recorded one.
    #[wasm_bindgen(js_name = hintHeight)]
    pub fn hint_height(&self) -> Option<f64> {
        self.hints.size.map(|(_, h)| h)
    }

    /// Dots per inch the document's writer rendered at, if it recorded one.
    ///
    /// Advisory here in a way it is not on the canvas client, which sizes a
    /// backing store from it. This one always draws at 96.
    #[wasm_bindgen(js_name = hintDpi)]
    pub fn hint_dpi(&self) -> Option<f64> {
        self.hints.dpi
    }
}

impl PlotDocument {
    /// Re-derive the theme for the current mode.
    fn apply_theme(&mut self) {
        let theme = if self.dark {
            self.base.clone().invert()
        } else {
            self.base.clone()
        };
        self.view.set_theme(theme);
    }
}

/// Read a document and draw it once, for a caller that keeps no handle.
///
/// Every call decodes the document again, so this is for a one-shot export
/// rather than for anything that resizes. [`PlotDocument`] is the type for
/// that.
#[wasm_bindgen(js_name = renderSvg)]
pub fn render_svg(
    doc: Vec<u8>,
    width: f64,
    height: f64,
    id_prefix: &str,
) -> Result<String, JsError> {
    Ok(PlotDocument::load(doc)?
        .to_svg(width, height, id_prefix)
        .svg)
}

/// Carry a crate error across to JS with its `Display` text intact.
fn to_js<E: std::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}
