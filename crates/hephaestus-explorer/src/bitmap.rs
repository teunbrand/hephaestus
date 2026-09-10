//! RGBA8 to a Windows bitmap Explorer will accept.

use windows::core::Result;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
};

/// A `BITMAPINFO` describing a 32-bit top-down image of this size.
///
/// The negative height is what makes it top-down. A positive one would flip
/// every image vertically, which is the classic first bug with a DIB.
pub fn top_down_info(width: u32, height: u32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Swizzle RGBA into the BGRA a `BI_RGB` 32-bit DIB actually holds.
///
/// Getting this wrong does not fail, it swaps red and blue in every image —
/// which on a plot reads as a subtly wrong palette rather than as breakage.
pub fn to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; rgba.len()];
    for (dst, src) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(rgba.as_chunks::<4>().0)
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    out
}

/// Build a 32-bit top-down bitmap holding `rgba`.
///
/// Two conversions happen here and both are easy to get wrong.
///
/// Byte order and row order are both handled by the helpers above; the notes
/// there are the ones worth reading.
///
/// Alpha is copied but Explorer is told to ignore it (`WTSAT_RGB`), since a
/// plot is rendered onto an opaque background. That also sidesteps the
/// question of whether `WTSAT_ARGB` wants premultiplied alpha.
pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<HBITMAP> {
    let expected = (width as usize) * (height as usize) * 4;
    debug_assert_eq!(rgba.len(), expected, "buffer is not width * height * 4");

    let info = top_down_info(width, height);

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a 32-bit DIB of these dimensions, and `bits`
    // receives a pointer to `width * height * 4` bytes owned by the bitmap.
    let bitmap = unsafe {
        CreateDIBSection(
            Some(HDC::default()),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            Some(HANDLE::default()),
            0,
        )?
    };
    if bits.is_null() {
        return Err(windows::core::Error::from_win32());
    }

    // SAFETY: the section above is exactly `expected` bytes.
    let destination = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), expected) };
    destination.copy_from_slice(&to_bgra(rgba));

    Ok(bitmap)
}
