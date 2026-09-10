//! The `IThumbnailProvider`: what a `.hep` looks like as an icon in Explorer.
//!
//! Paired with `IInitializeWithStream` rather than `IPersistFile` because that
//! is what lets Explorer host it in its low-privilege surrogate
//! (`dllhost.exe`) instead of loading it into Explorer itself — the
//! arrangement Microsoft documents as preferred, and the one where a crash
//! costs a thumbnail rather than the desktop.

use std::cell::RefCell;

use windows::core::{implement, Result, HRESULT};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_UNEXPECTED};
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::System::Com::IStream;
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithStream, IInitializeWithStream_Impl,
};
use windows::Win32::UI::Shell::{
    IThumbnailProvider, IThumbnailProvider_Impl, WTSAT_RGB, WTS_ALPHATYPE,
};

use crate::diagnostics::diag;
use crate::{bitmap, stream};

#[implement(IThumbnailProvider, IInitializeWithStream)]
pub struct ThumbnailProvider {
    /// The document's bytes, read on `Initialize`.
    ///
    /// `RefCell` because the COM methods take `&self` and this is
    /// single-threaded per instance — Explorer creates one object per file and
    /// does not share it.
    document: RefCell<Option<Vec<u8>>>,
}

impl Default for ThumbnailProvider {
    fn default() -> Self {
        Self {
            document: RefCell::new(None),
        }
    }
}

impl IInitializeWithStream_Impl for ThumbnailProvider_Impl {
    fn Initialize(&self, stream: windows_core::Ref<'_, IStream>, _mode: u32) -> Result<()> {
        let stream = stream.ok()?;
        let bytes = stream::read_all(stream)?;
        diag!("thumbnail: initialized with {} bytes", bytes.len());
        *self.document.borrow_mut() = Some(bytes);
        Ok(())
    }
}

impl IThumbnailProvider_Impl for ThumbnailProvider_Impl {
    fn GetThumbnail(
        &self,
        cx: u32,
        phbmp: *mut HBITMAP,
        pdwalpha: *mut WTS_ALPHATYPE,
    ) -> Result<()> {
        if phbmp.is_null() || pdwalpha.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let borrowed = self.document.borrow();
        let Some(document) = borrowed.as_deref() else {
            // `GetThumbnail` before `Initialize` is a caller error.
            return Err(E_UNEXPECTED.into());
        };

        // `cx` is the largest edge the thumbnail may have, which is exactly
        // what the shared renderer takes — so the aspect handling, the size
        // clamping and the "solve at the document's natural size" choice are
        // all the same code the Linux thumbnailer runs.
        diag!("thumbnail: rendering at {cx}px");
        let thumbnail = hephaestus_thumbnailer::render(document, cx).map_err(|error| {
            diag!("thumbnail: render failed: {error}");
            windows::core::Error::from(HRESULT(E_FAIL.0))
        })?;
        diag!("thumbnail: {}x{}", thumbnail.width, thumbnail.height);
        let bitmap = bitmap::from_rgba(thumbnail.width, thumbnail.height, &thumbnail.rgba)?;

        // SAFETY: both pointers were null-checked above; ownership of the
        // bitmap passes to the caller, which is the documented contract.
        unsafe {
            *phbmp = bitmap;
            *pdwalpha = WTSAT_RGB;
        }
        Ok(())
    }
}
