//! A plot document in, a thumbnail out.
//!
//! Two consumers, which is why this is a library and not just a binary:
//!
//! - **`src/main.rs`**, the freedesktop thumbnailer. That contract is a
//!   *command line* — a file in `/usr/share/thumbnailers/` names a program to
//!   run with a size, an input and an output — so on Linux a process is the
//!   whole integration. No plugin ABI, no registration beyond a data file.
//! - **A Windows `IThumbnailProvider`**, which is a COM DLL loaded into
//!   Explorer's surrogate and therefore cannot shell out to anything.
//!
//! Both want the same thing: pixels at a size somebody else chose.
//!
//! The layout is solved at the document's *natural* point size and the result
//! scaled to fit, rather than re-solved for the thumbnail's box. That is the
//! same choice the macOS preview makes, and for the same reason: the natural
//! size is what the plot was composed for, and a thumbnail is a picture of the
//! plot rather than the plot laid out for a 128-pixel window.

use hephaestus::backend::hybrid::HybridRenderer;
use hephaestus::document::{read_document, ReadContext};
use hephaestus::geometry::Size;
use hephaestus::scene::SceneBuilder;
use hephaestus::Renderer as _RendererTrait;

/// Points per inch, the unit a document's size hint is in.
const POINTS_PER_INCH: f64 = 72.0;

/// Natural size for a document that recorded no size hint.
const DEFAULT_SIZE: (f64, f64) = (600.0, 450.0);

/// Size hints outside this range are treated as absent rather than obeyed.
const SIZE_LIMITS: (f64, f64) = (36.0, 4000.0);

/// Largest thumbnail edge this will produce.
///
/// The freedesktop sizes are 128, 256, 512 and 1024; anything past this is a
/// caller mistake rather than a request worth honoring.
pub const MAX_EDGE: u32 = 4096;

/// A rendered thumbnail.
pub struct Thumbnail {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA8 with straight alpha, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Resolution the layout was solved against, for the PNG's `pHYs`.
    pub dpi: f64,
}

/// Anything that can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum ThumbnailError {
    /// The bytes are not a document this build can read.
    #[error("not a readable plot document: {0}")]
    Document(#[from] hephaestus::document::DocumentError),
    /// No usable GPU adapter, a frame too large, a readback that failed.
    #[error("rendering failed: {0}")]
    Backend(#[from] hephaestus::backend::BackendError),
    /// A size no thumbnail should be.
    #[error("{0} is not a usable thumbnail size (1..={MAX_EDGE})")]
    Size(u32),
}

/// Render `document` so that neither edge exceeds `max_edge` pixels.
///
/// Builds a renderer, uses it once and drops it, which is right for a
/// thumbnailer: one process per file, so there is nothing to reuse it across.
/// A caller that renders repeatedly — a preview pane being resized — wants
/// [`Renderer`] instead, since the adapter setup is most of the cost.
pub fn render(document: &[u8], max_edge: u32) -> Result<Thumbnail, ThumbnailError> {
    Renderer::new()?.render(document, max_edge)
}

/// A rasterizer held across several renders.
///
/// Acquiring a GPU adapter is the expensive part — measured at well over
/// 100 ms against a few milliseconds for the drawing — so anything that
/// renders the same document repeatedly should keep one of these. The Windows
/// preview handler does: it re-renders on every resize, which is what buys it
/// the reflow the macOS preview structurally cannot have.
pub struct Renderer {
    inner: HybridRenderer,
}

impl Renderer {
    /// Acquire an adapter and build a rasterizer.
    pub fn new() -> Result<Self, ThumbnailError> {
        Ok(Self {
            inner: HybridRenderer::new()?,
        })
    }

    /// Render `document` so that neither edge exceeds `max_edge` pixels.
    ///
    /// The aspect is the document's own, so the result is usually smaller than
    /// `max_edge` in one dimension — which is what every thumbnail consumer
    /// expects, and what stops a wide plot being letterboxed into a square.
    pub fn render(&mut self, document: &[u8], max_edge: u32) -> Result<Thumbnail, ThumbnailError> {
        self.render_boxed(document, max_edge, max_edge)
    }

    /// Render `document` to fit inside `max_width` × `max_height`.
    ///
    /// What a preview pane wants: the box is the pane, which is rarely square,
    /// and the plot should fill as much of it as its own aspect allows.
    pub fn render_boxed(
        &mut self,
        document: &[u8],
        max_width: u32,
        max_height: u32,
    ) -> Result<Thumbnail, ThumbnailError> {
        for edge in [max_width, max_height] {
            if edge == 0 || edge > MAX_EDGE {
                return Err(ThumbnailError::Size(edge));
            }
        }
        self.draw(document, max_width, max_height)
    }

    fn draw(
        &mut self,
        document: &[u8],
        max_width: u32,
        max_height: u32,
    ) -> Result<Thumbnail, ThumbnailError> {
        let read = read_document(document, ReadContext::builtin())?;

        let mut composition = read.composition;
        let (natural_width, natural_height) =
            read.hints.size.filter(usable).unwrap_or(DEFAULT_SIZE);

        // One scale for both axes: pixels per point, chosen so the plot fits
        // the box in whichever direction is tighter. The layout is then solved
        // at the natural point extent and rasterized at this density, which is
        // a sharper or coarser picture of the same plot rather than a
        // different one.
        let scale =
            (f64::from(max_width) / natural_width).min(f64::from(max_height) / natural_height);
        let width = ((natural_width * scale).round() as u32).clamp(1, max_width);
        let height = ((natural_height * scale).round() as u32).clamp(1, max_height);
        let dpi = POINTS_PER_INCH * scale;

        let background = read
            .hints
            .background
            .unwrap_or(composition.theme_ref().palette.paper)
            .with_alpha(1.0);

        self.inner.scene().clear();
        composition.render(
            self.inner.scene(),
            Size::new(f64::from(width), f64::from(height)),
            dpi,
        );

        let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
        self.inner
            .render_to_buffer(width, height, background, &mut rgba)?;

        Ok(Thumbnail {
            width,
            height,
            rgba,
            dpi,
        })
    }
}

/// Encode a thumbnail as a PNG.
///
/// `PngCompression::Fast` rather than balanced: a thumbnailer is on a file
/// manager's critical path, the image is small, and the difference in bytes at
/// this size is not worth the milliseconds.
pub fn encode_png(thumbnail: &Thumbnail) -> std::io::Result<Vec<u8>> {
    hephaestus::png::encode_png(
        thumbnail.width,
        thumbnail.height,
        &thumbnail.rgba,
        hephaestus::png::PngCompression::Fast,
        Some(thumbnail.dpi),
    )
}

/// Whether a size hint is one worth obeying.
fn usable(size: &(f64, f64)) -> bool {
    let (width, height) = *size;
    [width, height]
        .iter()
        .all(|value| value.is_finite() && *value >= SIZE_LIMITS.0 && *value <= SIZE_LIMITS.1)
}
