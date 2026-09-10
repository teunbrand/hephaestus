//! A way for a shell extension to say something.
//!
//! Both handlers run inside a surrogate process the tester did not start —
//! `dllhost.exe` for the thumbnail, `prevhost.exe` for the preview — so there
//! is no stdout, no stderr and no console. `OutputDebugStringW` is the one
//! channel that works from there: run **DebugView** elevated with *Capture
//! Global Win32* on, and the lines appear.
//!
//! Off unless built with `--features diagnostics`, and the argument is a
//! closure so nothing is formatted when it is off.

/// Write a line, if this build has diagnostics.
#[cfg(feature = "diagnostics")]
pub fn write(message: impl FnOnce() -> String) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;

    let line: Vec<u16> = format!("[hephaestus] {}\r\n", message())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: a NUL-terminated wide string, which is the whole contract.
    unsafe { OutputDebugStringW(PCWSTR(line.as_ptr())) };
}

/// Without the feature this is a no-op, and the closure is never called — so
/// a release build formats nothing.
#[cfg(not(feature = "diagnostics"))]
pub fn write(_message: impl FnOnce() -> String) {}

/// Report a line to whatever is listening.
macro_rules! diag {
    ($($arg:tt)*) => {
        $crate::diagnostics::write(|| format!($($arg)*))
    };
}

pub(crate) use diag;
