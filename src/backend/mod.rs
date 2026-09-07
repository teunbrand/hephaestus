//! Backend trait and error types.

use crate::color::Color;
use crate::scene::SceneBuilder;

#[cfg(any(feature = "vello", feature = "vello-hybrid", feature = "webgl"))]
mod convert;
// Mesh decomposition works in this crate's own types and emits plain
// `SceneBuilder::fill` calls, so every backend can share it — including
// the ones with no GPU.
#[cfg(any(
    feature = "vello",
    feature = "vello-hybrid",
    feature = "webgl",
    feature = "svg"
))]
mod mesh;

#[cfg(feature = "vello")]
pub mod vello;

#[cfg(any(feature = "vello-hybrid", feature = "webgl"))]
pub mod hybrid;

#[cfg(feature = "svg")]
pub mod svg;

#[cfg(feature = "pdf")]
pub mod pdf;

// The link-safety allow-list, shared by the two backends that emit a
// clickable destination. Two copies of a security check is how they
// drift.
#[cfg(any(feature = "svg", feature = "pdf"))]
mod href;

/// Largest 2D texture dimension this crate asks a device for, in pixels.
///
/// wgpu's default limit is 8192 and the sparse-strip rasterizer caps the
/// intermediate textures it allocates for layers at 4096 unless told
/// otherwise, both of which a print-resolution export passes. Asking for more
/// lifts the ceiling to whatever the hardware reports, up to this; staying
/// inside `u16` matters because that is what the rasterizer sizes those
/// textures in.
#[cfg(any(feature = "vello", feature = "vello-hybrid", feature = "webgl"))]
pub const MAX_TEXTURE_DIMENSION: u32 = 16384;

/// Default wgpu limits with the 2D texture dimension raised as far as
/// `adapter` allows, up to [`MAX_TEXTURE_DIMENSION`].
///
/// Requesting more than an adapter reports fails device creation, so the ask
/// is clamped to what it offers rather than fixed.
#[cfg(any(feature = "vello", feature = "vello-hybrid"))]
pub fn device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    wgpu::Limits {
        max_texture_dimension_2d: adapter
            .limits()
            .max_texture_dimension_2d
            .min(MAX_TEXTURE_DIMENSION),
        ..wgpu::Limits::default()
    }
}

/// Reject a frame no texture on `device` could hold, before anything is
/// allocated for it.
///
/// wgpu validates a texture's dimensions by panicking, and a rasterizing
/// backend sizes its target from the frame, so both ends of the range are
/// checked here and reported as an error instead: zero, which no texture can
/// be, and past `max_texture_dimension_2d`, which is as much as this device
/// grants — see [`device_limits`] for how much that is asked to be.
#[cfg(any(feature = "vello", feature = "vello-hybrid"))]
pub(crate) fn check_frame_size(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<(), BackendError> {
    if width == 0 || height == 0 {
        return Err(BackendError::Other(
            "cannot render a zero-sized frame".into(),
        ));
    }
    let max = device.limits().max_texture_dimension_2d;
    if width > max || height > max {
        return Err(BackendError::Other(format!(
            "frame {width}×{height} exceeds the {max} px this device's textures allow"
        )));
    }
    Ok(())
}

/// Owns backend resources (GPU device, pipelines, etc.) and rasterizes a scene
/// to an RGBA8 buffer.
///
/// `SceneBuilder` is the authoring surface; this trait is the "produce output"
/// step. Split into two traits because authoring is pure CPU/infallible and
/// rasterization is fallible/resource-owning — and the recording backend only
/// needs `SceneBuilder`.
pub trait Renderer {
    /// Concrete scene type for this backend. Implements [`SceneBuilder`].
    type Scene: SceneBuilder;

    /// Mutable access to the scene being built. Issue draw calls against this.
    fn scene(&mut self) -> &mut Self::Scene;

    /// Render the current scene into `out`, which must be exactly
    /// `width * height * 4` bytes.
    ///
    /// Pixels are RGBA8 with **straight (un-premultiplied) alpha** — the
    /// convention PNG and most CPU-side image libraries expect. Callers
    /// feeding the buffer to a compositor that assumes premultiplied alpha
    /// (CoreGraphics, Skia, Cairo, a SrcOver blend on the GPU) must
    /// premultiply first.
    fn render_to_buffer(
        &mut self,
        width: u32,
        height: u32,
        background: Color,
        out: &mut [u8],
    ) -> Result<(), BackendError>;
}

/// Optional extension for wgpu-backed renderers: rasterise directly into a
/// host-owned texture, bypassing the CPU readback that
/// [`Renderer::render_to_buffer`] uses.
///
/// This is the path for showing a scene in a window. A host that owns its own
/// wgpu device, queue, and presentation surface constructs the backend with a
/// `with_device`-style constructor so all GPU work shares a single device,
/// then calls [`render_to_texture`](Self::render_to_texture) each frame. The
/// `window` feature is this crate's own implementation of that host; see
/// [`crate::window`].
///
/// **Target constraints.** The supplied `view` must wrap a texture with
/// format `Rgba8Unorm` and usage including [`Self::REQUIRED_TARGET_USAGE`],
/// which the backend states because backends differ: a compute-shader
/// rasteriser writes through `STORAGE_BINDING`, a render-pipeline one needs
/// `RENDER_ATTACHMENT`. Either way a swap-chain texture cannot serve as the
/// direct target, since it carries neither the right format nor, for the
/// compute path, the right usage. Whatever the host does with the result adds
/// its own flag: `TEXTURE_BINDING` to blit the view onto a surface,
/// `COPY_SRC` to copy it back. Hosts whose presentation surface uses a
/// different format (typical for swap chains) are responsible for blitting
/// from this view to the surface.
///
/// **Alpha.** Which convention the view receives is backend-defined and
/// stated by [`Self::TARGET_IS_PREMULTIPLIED`]; unlike
/// [`Renderer::render_to_buffer`], this path does not normalise it. A host
/// presenting translucent content, or blending the view through a SrcOver
/// pipeline, has to consult that flag and convert in its blit shader. Opaque
/// content is unaffected — the two conventions coincide at alpha 255.
///
/// **Picking.** Picking (when enabled at construction) still rasterises the
/// parallel pick scene into the backend's own pick target and reads it back
/// to CPU, so [`pick_at`](crate::backend::vello::VelloRenderer::pick_at)
/// remains valid after a `render_to_texture` call.
#[cfg(any(feature = "vello", feature = "vello-hybrid"))]
pub trait WgpuRenderer: Renderer {
    /// Usage flags the texture behind `view` must carry.
    ///
    /// Backends disagree about this, so a host allocating the target asks
    /// instead of assuming: a compute-shader rasteriser writes through a
    /// storage binding, a render-pipeline one needs a colour attachment. The
    /// host unions in whatever its own use of the result needs —
    /// `TEXTURE_BINDING` to blit from it, `COPY_SRC` to read it back.
    const REQUIRED_TARGET_USAGE: wgpu::TextureUsages;

    /// Whether [`render_to_texture`](Self::render_to_texture) leaves
    /// premultiplied alpha in the target.
    ///
    /// Backends disagree here too, and for the same reason — it follows from
    /// how the rasteriser composites. A host blending the result has to know:
    /// treating premultiplied content as straight (or the reverse) shifts
    /// every partially transparent pixel. It makes no difference when the
    /// content is opaque, which is how the in-crate window host presents
    /// under either backend.
    ///
    /// [`Renderer::render_to_buffer`] is not affected — that path always
    /// converts to straight alpha, whatever the rasteriser does.
    const TARGET_IS_PREMULTIPLIED: bool;

    /// Render the current scene into `view`. See trait docs for the
    /// format / usage requirements `view` must satisfy.
    fn render_to_texture(
        &mut self,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        background: Color,
    ) -> Result<(), BackendError>;
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("output buffer is the wrong size (expected {expected} bytes, got {actual})")]
    BufferSize { expected: usize, actual: usize },

    #[error("no compatible GPU adapter available")]
    NoAdapter,

    #[error("failed to acquire GPU device: {0}")]
    DeviceRequest(String),

    #[error("GPU readback failed: {0}")]
    Readback(String),

    #[error(
        "scene exceeds the backend's draw capacity ({used} draw-info words, max {max}); \
         draw fewer objects or split the scene across passes"
    )]
    SceneTooLarge {
        /// Draw-info words the scene occupies.
        used: u32,
        /// Largest count the backend can rasterise in one pass.
        max: u32,
    },

    #[error("backend internal error: {0}")]
    Other(String),
}
