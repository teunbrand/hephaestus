//! The `IPreviewHandler`: what a `.hep` looks like in Explorer's Preview Pane
//! (Alt+P).
//!
//! **This is the one shell integration in the repo that can reflow.** Unlike
//! Quick Look's data-based reply, which is a fixed-size document handed over
//! once, a preview handler is given an `HWND` and told when its rectangle
//! changes — so the plot can be re-solved at the new size, which is the whole
//! point of shipping a document rather than a picture. See
//! `../hephaestus-quicklook/CLAUDE.md` for why macOS cannot do this.
//!
//! Runs in `prevhost.exe` rather than in Explorer, which is what the `AppID`
//! on the class registration arranges. A crash therefore costs a preview.

use std::cell::RefCell;

use windows::core::{implement, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, E_FAIL, E_INVALIDARG, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    SetDIBitsToDevice, DIB_RGB_COLORS, HBRUSH, PAINTSTRUCT,
};
use windows::Win32::System::Com::IStream;
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithStream, IInitializeWithStream_Impl,
};
use windows::Win32::UI::Shell::{IPreviewHandler, IPreviewHandler_Impl};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, MoveWindow,
    RegisterClassExW, SetWindowLongPtrW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, MSG,
    WINDOW_EX_STYLE, WM_ERASEBKGND, WM_PAINT, WNDCLASSEXW, WS_CHILD, WS_VISIBLE,
};

use crate::bitmap;
use crate::stream;

/// Window class this registers once, lazily.
const CLASS_NAME: PCWSTR = windows::core::w!("HephaestusPreviewPane");

/// Pixels of margin around the plot inside the pane.
const MARGIN: i32 = 8;

/// What the window procedure needs, separate from the COM object so a raw
/// pointer to it can live in the window's user data.
#[derive(Default)]
struct Pane {
    /// BGRA, top-down, ready for `SetDIBitsToDevice`.
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    /// Painted around the plot, so the pane matches the document rather than
    /// showing the shell's default grey behind a centred picture.
    background: COLORREF,
}

#[implement(IPreviewHandler, IInitializeWithStream)]
pub struct PreviewHandler {
    document: RefCell<Option<Vec<u8>>>,
    /// Kept across resizes: acquiring an adapter is most of the cost, and this
    /// re-renders on every `SetRect`.
    renderer: RefCell<Option<hephaestus_thumbnailer::Renderer>>,
    parent: RefCell<HWND>,
    bounds: RefCell<RECT>,
    window: RefCell<HWND>,
    /// Boxed so its address is stable while the window holds it.
    pane: RefCell<Box<Pane>>,
}

impl Default for PreviewHandler {
    fn default() -> Self {
        Self {
            document: RefCell::new(None),
            renderer: RefCell::new(None),
            parent: RefCell::new(HWND::default()),
            bounds: RefCell::new(RECT::default()),
            window: RefCell::new(HWND::default()),
            pane: RefCell::new(Box::new(Pane::default())),
        }
    }
}

impl IInitializeWithStream_Impl for PreviewHandler_Impl {
    fn Initialize(&self, source: windows_core::Ref<'_, IStream>, _mode: u32) -> Result<()> {
        *self.document.borrow_mut() = Some(stream::read_all(source.ok()?)?);
        Ok(())
    }
}

impl IPreviewHandler_Impl for PreviewHandler_Impl {
    fn SetWindow(&self, parent: HWND, bounds: *const RECT) -> Result<()> {
        if bounds.is_null() {
            return Err(E_INVALIDARG.into());
        }
        *self.parent.borrow_mut() = parent;
        // SAFETY: null-checked above.
        *self.bounds.borrow_mut() = unsafe { *bounds };
        self.reposition();
        Ok(())
    }

    fn SetRect(&self, bounds: *const RECT) -> Result<()> {
        if bounds.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: null-checked above.
        *self.bounds.borrow_mut() = unsafe { *bounds };
        self.reposition();
        // The size changed, so the layout is stale — this is the call that
        // makes the preview reflow rather than scale.
        self.redraw();
        Ok(())
    }

    fn DoPreview(&self) -> Result<()> {
        if self.document.borrow().is_none() {
            return Err(E_FAIL.into());
        }
        self.ensure_window()?;
        self.redraw();
        Ok(())
    }

    fn Unload(&self) -> Result<()> {
        let window = std::mem::take(&mut *self.window.borrow_mut());
        if !window.is_invalid() {
            // SAFETY: a window this object created and has not yet destroyed.
            unsafe {
                let _ = SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                let _ = DestroyWindow(window);
            }
        }
        *self.document.borrow_mut() = None;
        *self.renderer.borrow_mut() = None;
        **self.pane.borrow_mut() = Pane::default();
        Ok(())
    }

    fn SetFocus(&self) -> Result<()> {
        let window = *self.window.borrow();
        if window.is_invalid() {
            return Err(E_FAIL.into());
        }
        // SAFETY: a live child window.
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(window));
        }
        Ok(())
    }

    fn QueryFocus(&self) -> Result<HWND> {
        // SAFETY: no arguments, and a null result is a valid answer meaning
        // nothing in this thread has focus.
        Ok(unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() })
    }

    fn TranslateAccelerator(&self, _message: *const MSG) -> Result<()> {
        // Nothing here is interactive, so every accelerator belongs to the
        // host. `S_FALSE` is the documented way to say so, and returning it
        // as an error is how a non-`S_OK` success is expressed here.
        Err(windows::core::Error::from(HRESULT(1)))
    }
}

impl PreviewHandler_Impl {
    /// Create the child window if it does not exist yet.
    fn ensure_window(&self) -> Result<()> {
        if !self.window.borrow().is_invalid() {
            return Ok(());
        }
        let parent = *self.parent.borrow();
        if parent.is_invalid() {
            return Err(E_FAIL.into());
        }
        register_class();

        let bounds = *self.bounds.borrow();
        // SAFETY: a child of a window the host supplied, with a class
        // registered above.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS_NAME,
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                Some(parent),
                None,
                None,
                None,
            )?
        };

        // The window procedure reaches the pixels through this pointer. The
        // `Box` keeps the address stable; `Unload` clears it before the window
        // goes away.
        let pane: *mut Pane = &mut **self.pane.borrow_mut();
        // SAFETY: the window was just created and the pointer outlives it,
        // being owned by this object and cleared in `Unload`.
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, pane as isize);
        }
        *self.window.borrow_mut() = window;
        Ok(())
    }

    /// Move the child window to the host's current rectangle.
    fn reposition(&self) {
        let window = *self.window.borrow();
        if window.is_invalid() {
            return;
        }
        let bounds = *self.bounds.borrow();
        // SAFETY: a live child window.
        unsafe {
            let _ = MoveWindow(
                window,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                true,
            );
        }
    }

    /// Re-render at the current size and ask for a repaint.
    ///
    /// Failure is deliberately silent: a preview that cannot be drawn should
    /// leave the pane in whatever state it was, not raise an error at the
    /// shell.
    fn redraw(&self) {
        let window = *self.window.borrow();
        if window.is_invalid() {
            return;
        }
        let bounds = *self.bounds.borrow();
        let width = (bounds.right - bounds.left - 2 * MARGIN).max(1) as u32;
        let height = (bounds.bottom - bounds.top - 2 * MARGIN).max(1) as u32;

        let borrowed = self.document.borrow();
        let Some(document) = borrowed.as_deref() else {
            return;
        };

        let mut renderer = self.renderer.borrow_mut();
        if renderer.is_none() {
            *renderer = hephaestus_thumbnailer::Renderer::new().ok();
        }
        let Some(renderer) = renderer.as_mut() else {
            return;
        };
        let Ok(rendered) = renderer.render_boxed(document, width, height) else {
            return;
        };

        {
            let mut pane = self.pane.borrow_mut();
            pane.pixels = bitmap::to_bgra(&rendered.rgba);
            pane.width = rendered.width;
            pane.height = rendered.height;
            // The plot's own paper colour, read back from the corner pixel so
            // the margin matches whatever the document asked for.
            let corner = &rendered.rgba[..4];
            pane.background = COLORREF(
                u32::from(corner[0]) | (u32::from(corner[1]) << 8) | (u32::from(corner[2]) << 16),
            );
        }

        // SAFETY: a live child window.
        unsafe {
            let _ = InvalidateRect(Some(window), None, false);
        }
    }
}

/// Register the window class once per process.
fn register_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialized and names a procedure with the
        // right signature. A duplicate registration is harmless.
        unsafe {
            RegisterClassExW(&class);
        }
    });
}

/// Paints the pane, and nothing else.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Everything is painted below, so letting the shell erase first would
        // only flicker.
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(window);
            LRESULT(0)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

/// Blit the last render, centred, with the plot's own colour around it.
unsafe fn paint(window: HWND) {
    let pane = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Pane;
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(window, &mut ps);

    if let Some(pane) = pane.as_ref() {
        if !pane.pixels.is_empty() {
            let brush: HBRUSH = CreateSolidBrush(pane.background);
            FillRect(hdc, &ps.rcPaint, brush);
            let _ = DeleteObject(brush.into());

            let client_width = ps.rcPaint.right.max(0);
            let client_height = ps.rcPaint.bottom.max(0);
            let x = ((client_width - pane.width as i32) / 2).max(0);
            let y = ((client_height - pane.height as i32) / 2).max(0);
            let info = bitmap::top_down_info(pane.width, pane.height);
            SetDIBitsToDevice(
                hdc,
                x,
                y,
                pane.width,
                pane.height,
                0,
                0,
                0,
                pane.height,
                pane.pixels.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
            );
        }
    }

    let _ = EndPaint(window, &ps);
}
