//! Draining an `IStream`, shared by both handlers.

use windows::core::Result;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::IStream;

/// Largest document either handler will read.
///
/// Neither a thumbnail nor a preview is the place to page in an arbitrarily
/// large file, and a `.hep` carrying embedded images is the only way to get
/// near this.
const MAX_DOCUMENT: usize = 64 * 1024 * 1024;

/// How much to ask for per `Read`.
const CHUNK: usize = 64 * 1024;

/// Drain a stream into memory.
///
/// `Read` in a loop rather than seeking for a size: a stream is not required
/// to be seekable, and at these sizes growing a `Vec` costs nothing worth
/// avoiding.
pub fn read_all(stream: &IStream) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let mut read: u32 = 0;
        // SAFETY: `chunk` is `CHUNK` writable bytes; `read` receives the count
        // actually written.
        unsafe {
            stream
                .Read(
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                    Some(&mut read),
                )
                .ok()?;
        }
        if read == 0 {
            break;
        }
        if out.len() + read as usize > MAX_DOCUMENT {
            return Err(E_FAIL.into());
        }
        out.extend_from_slice(&chunk[..read as usize]);
    }
    Ok(out)
}
