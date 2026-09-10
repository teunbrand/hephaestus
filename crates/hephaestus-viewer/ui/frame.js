// The frame wire format, read back.
//
// The other half of `src/render/frame.rs`. A frame arrives as one opaque
// buffer — a fixed 32-byte header, then either raw RGBA8 or a PNG — because a
// picture is millions of pixels and JSON is not the way to move them. Keep the
// two files in step: `tests/frame_header.rs` pins the layout from the Rust
// side, and nothing here would fail loudly if they drifted.

export const HEADER_LEN = 32;

const MAGIC = 0x46504548; // "HEPF", little-endian

export const STATUS_OK = 0;
export const STATUS_SUPERSEDED = 1;
export const STATUS_ERROR = 2;
export const STATUS_NO_SUCH_TAB = 3;

const FLAG_PNG = 1 << 0;
const FLAG_DRAFT = 1 << 1;

/**
 * Normalize whatever `invoke` handed back into a byte view.
 *
 * A command returning `tauri::ipc::Response` arrives as an `ArrayBuffer`, but
 * the array fallback is worth handling: it is what a transport that could not
 * carry raw bytes would produce, and the difference would otherwise show up as
 * a header that fails to parse.
 */
export function asBytes(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  if (Array.isArray(value)) return new Uint8Array(value);
  throw new Error('the frame did not arrive as bytes');
}

/** Read the header, or throw if the buffer is not a frame. */
export function parseHeader(bytes) {
  if (bytes.byteLength < HEADER_LEN) {
    throw new Error(`a frame is at least ${HEADER_LEN} bytes, got ${bytes.byteLength}`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(0, true) !== MAGIC) {
    throw new Error('not a frame buffer');
  }
  const version = view.getUint16(4, true);
  if (version !== 1) {
    throw new Error(`frame format version ${version} is not the one this build reads`);
  }
  const flags = view.getUint8(6);
  return {
    status: view.getUint8(7),
    png: (flags & FLAG_PNG) !== 0,
    draft: (flags & FLAG_DRAFT) !== 0,
    width: view.getUint32(8, true),
    height: view.getUint32(12, true),
    dpi: view.getUint32(16, true) / 1000,
    tab: view.getUint32(20, true),
    seq: view.getUint32(24, true),
    payloadLength: view.getUint32(28, true),
  };
}

/** The bytes after the header. */
export function payload(bytes, header) {
  const start = bytes.byteOffset + HEADER_LEN;
  const length = Math.min(header.payloadLength, bytes.byteLength - HEADER_LEN);
  return new Uint8Array(bytes.buffer, start, length);
}

/** The error message an unsuccessful frame carries. */
export function message(bytes, header) {
  return new TextDecoder().decode(payload(bytes, header));
}

/**
 * Put a frame on a canvas.
 *
 * The backing store is resized in the same synchronous block as the draw,
 * which is the whole trick: assigning `canvas.width` clears the buffer, so
 * doing it a paint before the pixels arrive shows a blank canvas for a frame.
 * Leaving the old picture in place and letting CSS stretch it until the new
 * one lands reads as a smooth resize instead. The canvas is never cleared and
 * never sized to something it has no pixels for.
 *
 * Async only because a PNG has to be decoded, and that decode happens off the
 * main thread — which is the other half of why encoding is worth it where the
 * transport is slow.
 */
export async function paint(canvas, context, bytes, header) {
  if (header.png) {
    const blob = new Blob([payload(bytes, header)], { type: 'image/png' });
    const bitmap = await createImageBitmap(blob);
    try {
      resizeTo(canvas, header);
      context.drawImage(bitmap, 0, 0);
    } finally {
      bitmap.close();
    }
    return;
  }

  const pixels = header.width * header.height * 4;
  const start = bytes.byteOffset + HEADER_LEN;
  // `Uint8ClampedArray` refuses an offset that is not a multiple of four, and
  // the header is sized to keep it aligned — but only if the buffer itself
  // started aligned, which is not something the transport promises.
  const view =
    start % 4 === 0
      ? new Uint8ClampedArray(bytes.buffer, start, pixels)
      : new Uint8ClampedArray(bytes.slice(HEADER_LEN, HEADER_LEN + pixels));
  const image = new ImageData(view, header.width, header.height);
  resizeTo(canvas, header);
  context.putImageData(image, 0, 0);
}

/** Size the backing store to the picture, without clearing it needlessly. */
function resizeTo(canvas, header) {
  if (canvas.width !== header.width) canvas.width = header.width;
  if (canvas.height !== header.height) canvas.height = header.height;
}
