//! Writing an open document out as a file.
//!
//! Every format starts from the same place — a composition read back from the
//! document's own bytes — and differs only in what it is handed to. The
//! re-read is not an optimization to remove: drawing a composition leaves a
//! solved layout cached against the size it was drawn at, and an export is
//! almost never the size on screen.

use std::path::Path;

use hephaestus::backend::hybrid::HybridRenderer;
use hephaestus::backend::MAX_TEXTURE_DIMENSION;
use hephaestus::geometry::Size;
use hephaestus::image::{encode_jpeg, encode_tiff, encode_webp, TiffCompression};
use hephaestus::pdf::{encode_pdf, PdfConfig, PdfScene};
use hephaestus::png::{encode_png, PngCompression};
use hephaestus::scene::SceneBuilder;
use hephaestus::svg::{encode_svg, SvgConfig, SvgScene, SvgUnits, TextMode};
use hephaestus::Renderer;

use super::doc::OpenDoc;
use crate::error::ViewerError;

/// Points per inch. The unit every size request is normalized through, since
/// it is the unit a plot's own lengths are expressed in.
const POINTS_PER_INCH: f64 = 72.0;

/// What a file can be written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Png,
    Jpeg,
    Tiff,
    Webp,
    Svg,
    Pdf,
}

impl ExportFormat {
    /// Filename extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tiff",
            Self::Webp => "webp",
            Self::Svg => "svg",
            Self::Pdf => "pdf",
        }
    }

    /// Name for a file dialog's format filter.
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG image",
            Self::Jpeg => "JPEG image",
            Self::Tiff => "TIFF image",
            Self::Webp => "WebP image",
            Self::Svg => "SVG drawing",
            Self::Pdf => "PDF document",
        }
    }

    /// Whether the format holds pixels, and therefore needs the GPU and a
    /// resolution.
    pub fn is_raster(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg | Self::Tiff | Self::Webp)
    }
}

/// Unit the requested width and height are expressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeUnit {
    /// Pixels, resolved against the requested dpi.
    Px,
    /// Inches.
    In,
    /// Millimeters.
    Mm,
    /// Points.
    Pt,
}

impl SizeUnit {
    /// How many inches one of these is.
    fn inches(self, dpi: f64) -> f64 {
        match self {
            Self::Px => 1.0 / dpi,
            Self::In => 1.0,
            Self::Mm => 1.0 / 25.4,
            Self::Pt => 1.0 / POINTS_PER_INCH,
        }
    }
}

/// A request to write the current document out.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ExportSpec {
    /// Which writer to use.
    pub format: ExportFormat,
    /// Requested width, in [`Self::unit`].
    pub width: f64,
    /// Requested height, in [`Self::unit`].
    pub height: f64,
    /// What `width` and `height` mean.
    pub unit: SizeUnit,
    /// Resolution for a raster format, and the scale a pixel size is resolved
    /// against for any format.
    pub dpi: f64,
    /// JPEG quality, 1–100. Ignored by every other format.
    pub jpeg_quality: u8,
    /// Emit SVG text as filled outlines rather than as `<text>` elements.
    /// Loses editability and selectability; gains a file that needs no font.
    pub outline_text: bool,
    /// Embed the fonts an SVG's `<text>` refers to.
    pub embed_fonts: bool,
}

/// What was written.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportReport {
    /// Where it landed.
    pub path: String,
    /// Format it was written in.
    pub format: ExportFormat,
    /// Pixel width for a raster format; rounded point width for a vector one.
    pub width: u32,
    /// Pixel height for a raster format; rounded point height for a vector one.
    pub height: u32,
    /// Resolution the layout was solved against.
    pub dpi: f64,
    /// Size of the file.
    pub bytes: u64,
    /// Whatever the writer could not express exactly.
    ///
    /// The vector backends report degradation rather than failing, so this is
    /// the only place a caller learns that a blend mode was flattened or a
    /// font could not be embedded.
    pub warnings: Vec<String>,
}

/// The size request, normalized to the one unit that decides layout.
#[derive(Debug, Clone, Copy)]
struct Extent {
    width_pt: f64,
    height_pt: f64,
}

impl Extent {
    /// Resolve a spec's width and height into points.
    fn resolve(spec: &ExportSpec) -> Result<Self, ViewerError> {
        if !spec.dpi.is_finite() || spec.dpi <= 0.0 {
            return Err(ViewerError::Rejected(format!(
                "{} is not a usable resolution",
                spec.dpi
            )));
        }
        let per_unit = spec.unit.inches(spec.dpi) * POINTS_PER_INCH;
        let extent = Self {
            width_pt: spec.width * per_unit,
            height_pt: spec.height * per_unit,
        };
        for value in [extent.width_pt, extent.height_pt] {
            if !value.is_finite() || value <= 0.0 {
                return Err(ViewerError::Rejected(
                    "export width and height must both be positive".into(),
                ));
            }
        }
        Ok(extent)
    }

    /// Pixel dimensions at `dpi`, refused if no texture could hold them.
    ///
    /// Checked here rather than left to the renderer because the message a
    /// person needs names the limit, and because the alternative is
    /// discovering it after allocating most of a gigabyte.
    fn pixels(self, dpi: f64) -> Result<(u32, u32), ViewerError> {
        let scale = dpi / POINTS_PER_INCH;
        let w = (self.width_pt * scale).round();
        let h = (self.height_pt * scale).round();
        let max = f64::from(MAX_TEXTURE_DIMENSION);
        if w > max || h > max {
            return Err(ViewerError::Rejected(format!(
                "{w} × {h} px is past the {MAX_TEXTURE_DIMENSION} px a texture can hold; \
                 reduce the size or the resolution"
            )));
        }
        Ok(((w as u32).max(1), (h as u32).max(1)))
    }
}

/// Write `doc` to `path` as `spec` asks.
///
/// `renderer` is `None` where there is no working adapter, and only a raster
/// format needs one — which is the useful half of this being a `Renderer`
/// call rather than a backend the whole crate depends on. A machine that
/// cannot rasterize at all can still export SVG and PDF.
pub fn export(
    doc: &OpenDoc,
    renderer: Option<&mut HybridRenderer>,
    spec: &ExportSpec,
    path: &Path,
) -> Result<ExportReport, ViewerError> {
    let extent = Extent::resolve(spec)?;
    let mut comp = doc.fresh_composition()?;
    let background = doc.background();

    let (width, height, dpi, bytes, warnings) = if spec.format.is_raster() {
        // Size before renderer, so a request no device could satisfy is
        // reported as the size problem it is rather than as a missing
        // adapter — the second message is true and names the wrong cause.
        let (w, h) = extent.pixels(spec.dpi)?;
        let renderer = renderer.ok_or_else(|| {
            ViewerError::Rejected(format!(
                "cannot write a {} without a working GPU adapter; SVG and PDF need none",
                spec.format.label()
            ))
        })?;
        let size = Size::new(f64::from(w), f64::from(h));

        renderer.scene().clear();
        comp.render(renderer.scene(), size, spec.dpi);
        let mut pixels = vec![0u8; (w as usize) * (h as usize) * 4];
        renderer.render_to_buffer(w, h, background, &mut pixels)?;

        let encoded = match spec.format {
            ExportFormat::Png => {
                encode_png(w, h, &pixels, PngCompression::Balanced, Some(spec.dpi))
            }
            ExportFormat::Jpeg => encode_jpeg(
                w,
                h,
                &pixels,
                spec.jpeg_quality.clamp(1, 100),
                background,
                Some(spec.dpi),
            ),
            ExportFormat::Tiff => {
                encode_tiff(w, h, &pixels, TiffCompression::Deflate, Some(spec.dpi))
            }
            ExportFormat::Webp => encode_webp(w, h, &pixels, Some(spec.dpi)),
            ExportFormat::Svg | ExportFormat::Pdf => unreachable!("not a raster format"),
        }
        .map_err(ViewerError::Encode)?;

        (w, h, spec.dpi, encoded, Vec::new())
    } else {
        // Both vector writers are handed points and a dpi of 72, so a length
        // in the theme, a coordinate in the file and the page's own size are
        // all the same unit. For PDF that makes the size *be* the MediaBox;
        // for SVG it makes `SvgUnits::Pt` write a physical size whose numbers
        // match the `viewBox` rather than needing to be converted into it.
        let size = Size::new(extent.width_pt, extent.height_pt);
        let dpi = POINTS_PER_INCH;

        let (encoded, warnings) = match spec.format {
            ExportFormat::Svg => {
                let config = SvgConfig::new()
                    .background(Some(background))
                    .units(SvgUnits::Pt)
                    .pick_ids(false)
                    .text(if spec.outline_text {
                        TextMode::Outline
                    } else {
                        TextMode::Text
                    })
                    .embed_fonts(spec.embed_fonts);
                let mut scene = SvgScene::with_config(size, dpi, config);
                comp.render(&mut scene, size, dpi);
                let warnings = describe(scene.warnings());
                (encode_svg(&scene).into_bytes(), warnings)
            }
            ExportFormat::Pdf => {
                let config = PdfConfig::new().background(Some(background));
                let mut scene = PdfScene::with_config(size, dpi, config);
                comp.render(&mut scene, size, dpi);
                let warnings = describe(scene.warnings());
                (encode_pdf(&scene), warnings)
            }
            _ => unreachable!("not a vector format"),
        };

        (
            extent.width_pt.round() as u32,
            extent.height_pt.round() as u32,
            dpi,
            encoded,
            warnings,
        )
    };

    std::fs::write(path, &bytes).map_err(|e| ViewerError::io(path, e))?;

    Ok(ExportReport {
        path: path.display().to_string(),
        format: spec.format,
        width,
        height,
        dpi,
        bytes: bytes.len() as u64,
        warnings,
    })
}

/// Turn a backend's warning list into text the export report can carry.
fn describe<W: std::fmt::Debug>(warnings: &[W]) -> Vec<String> {
    warnings.iter().map(|w| format!("{w:?}")).collect()
}
