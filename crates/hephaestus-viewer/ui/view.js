// The canvas, and the loop that keeps it filled.
//
// A view owns the size the window gives it, the frames it asks the render
// thread for, and the rule that only one of those is ever outstanding. It does
// *not* know which document it draws: one window shows one document, so Rust
// reads that off the window the request came from and the frontend never names
// it.
//
// The sizing conventions are the wasm client's, deliberately — device pixels
// for the buffer, `96 × devicePixelRatio` for the dpi — because that pair is
// what makes a theme length in points come out the right physical size on a
// high-density display, and because a document should look the same in a
// browser and here.
//
// What is *not* the wasm client's is when the drawing happens. There, a resize
// draws synchronously inside the `ResizeObserver` callback, so the cleared
// backing store is never painted. Here the pixels come from another thread and
// cannot arrive in the same turn, so the trick is inverted: the canvas is
// sized in CSS, its backing store is left alone until pixels exist, and the
// old frame stretches until the new one lands. See `frame.js`.

import {
  asBytes,
  message,
  paint,
  parseHeader,
  STATUS_ERROR,
  STATUS_NO_SUCH_TAB,
  STATUS_SUPERSEDED,
} from './frame.js';

const { invoke } = window.__TAURI__.core;

/** CSS pixels per inch at ratio 1. The one place 96 appears. */
const BASE_DPI = 96;

/** How long a size has to hold still before the crisp frame is asked for. */
const SETTLE_MS = 150;

export class PlotView {
  constructor(canvas, { onError } = {}) {
    this.onError = onError;

    this.canvas = canvas;
    // A bare canvas reports 300×150, which would be a first frame at the
    // wrong aspect. One pixel is honest about having nothing yet, and the
    // first real frame replaces it before a paint.
    this.canvas.width = 1;
    this.canvas.height = 1;
    this.context = canvas.getContext('2d', {
      alpha: false,
      desynchronized: true,
    });

    this.seq = 0;
    // The size and resolution of, respectively: the frame being drawn right
    // now, and the frame currently on the canvas. Both are `null` when there
    // is none. Together they are what makes a redundant request cheap to
    // recognize — and there are two sources of those, a `ResizeObserver`
    // delivering its initial observation while the first frame is still in
    // flight, and a settle timer firing after a frame that was already crisp.
    this.sent = null;
    this.shown = null;
    this.queued = null;
    this.want = null;
    this.active = false;
    this.settleTimer = null;
    this.media = null;
    this.onMedia = null;

    this.observer = new ResizeObserver((entries) => this.#observed(entries));
  }

  /** Start watching the element's size and draw at whatever it is. */
  activate() {
    if (this.active) return;
    this.active = true;
    try {
      // The exact device-pixel box, so the backing store never drifts by a
      // rounding step against `devicePixelRatio`. Not universally supported —
      // `observe` throws where it is not.
      this.observer.observe(this.canvas, { box: 'device-pixel-content-box' });
    } catch {
      this.observer.observe(this.canvas);
    }
    this.#watchRatio();
    this.#sync();
  }

  /** Stop drawing. The last frame stays on the canvas for when it comes back. */
  deactivate() {
    if (!this.active) return;
    this.active = false;
    this.observer.disconnect();
    this.#unwatchRatio();
    clearTimeout(this.settleTimer);
    this.settleTimer = null;
  }

  /**
   * Forget that the canvas shows anything current.
   *
   * The document changed underneath a frame that is still correct in size, so
   * the size-based dedupe below would otherwise skip the redraw. An off-screen
   * view is invalidated rather than redrawn, and draws when it is next shown.
   */
  invalidate() {
    this.shown = null;
  }

  /** Ask for a fresh crisp frame — after a reload, or a theme change. */
  refresh() {
    this.invalidate();
    if (this.active) this.#request(false);
  }

  #observed(entries) {
    const entry = entries[entries.length - 1];
    const ratio = window.devicePixelRatio || 1;
    const exact = entry.devicePixelContentBoxSize?.[0];
    if (exact) {
      this.#resize(exact.inlineSize, exact.blockSize, ratio);
      return;
    }
    // Read the size off the entry rather than `clientWidth`, which would
    // force a layout flush inside the callback.
    const box = entry.contentBoxSize?.[0];
    const cssWidth = box ? box.inlineSize : this.canvas.clientWidth;
    const cssHeight = box ? box.blockSize : this.canvas.clientHeight;
    this.#resize(Math.round(cssWidth * ratio), Math.round(cssHeight * ratio), ratio);
  }

  #sync() {
    const ratio = window.devicePixelRatio || 1;
    const cssWidth = this.canvas.clientWidth;
    const cssHeight = this.canvas.clientHeight;
    if (!cssWidth || !cssHeight) return;
    this.#resize(Math.round(cssWidth * ratio), Math.round(cssHeight * ratio), ratio);
  }

  #resize(width, height, ratio) {
    this.want = {
      width: Math.max(1, width),
      height: Math.max(1, height),
      dpi: BASE_DPI * ratio,
    };

    // A draft only makes sense once there is something to stretch: the first
    // frame of a document goes straight to full resolution, so a newly opened
    // one is never briefly blurry — and needs no crisp frame chasing it.
    const draft = this.shown !== null;
    this.#request(draft);
    if (draft) this.#settle();
  }

  #settle() {
    clearTimeout(this.settleTimer);
    this.settleTimer = setTimeout(() => {
      this.settleTimer = null;
      this.#request(false);
    }, SETTLE_MS);
  }

  #request(draft) {
    if (!this.want) return;

    // Already on screen, at this exact size and no less crisply.
    if (covers(this.shown, this.want, draft)) return;

    if (this.sent) {
      // Already being drawn, at this exact size and no less crisply.
      if (covers(this.sent, this.want, draft)) return;
      // One outstanding frame per view, latest wins. A pending crisp request
      // is never downgraded to a draft by a later one, or letting go of the
      // mouse could leave a blurry picture on screen.
      this.queued = { draft: this.queued ? this.queued.draft && draft : draft };
      return;
    }

    const spec = {
      width: this.want.width,
      height: this.want.height,
      dpi: this.want.dpi,
      draft,
      seq: ++this.seq,
    };
    this.sent = spec;
    invoke('render_frame', { spec })
      .then((value) => this.#receive(value))
      .catch((error) => this.onError?.(String(error)))
      .finally(() => {
        this.sent = null;
        const queued = this.queued;
        this.queued = null;
        if (queued) this.#request(queued.draft);
      });
  }

  async #receive(value) {
    const bytes = asBytes(value);
    const header = parseHeader(bytes);
    // A reply for a request this view has already moved past. Cannot happen
    // while only one frame is outstanding, and cheap to be sure about.
    if (header.seq !== this.seq) return;

    // Superseded is the common case during a resize; no-such-tab means this
    // window's document has been closed. Both are silent.
    if (header.status === STATUS_SUPERSEDED || header.status === STATUS_NO_SUCH_TAB) {
      return;
    }
    if (header.status === STATUS_ERROR) {
      this.onError?.(message(bytes, header));
      return;
    }

    await paint(this.canvas, this.context, bytes, header);
    // Recorded from the header rather than from what was asked for, since a
    // draft comes back at half the size the request named.
    this.shown = {
      width: this.sent?.width ?? header.width,
      height: this.sent?.height ?? header.height,
      dpi: this.sent?.dpi ?? header.dpi,
      draft: header.draft,
    };
  }

  /**
   * Notice a move between monitors of different densities.
   *
   * Such a move changes `devicePixelRatio` without necessarily changing the
   * element's CSS box, so the resize observer may never fire. The query is
   * re-armed on each change because it names the ratio it was created for.
   */
  #watchRatio() {
    this.#unwatchRatio();
    const ratio = window.devicePixelRatio || 1;
    this.media = window.matchMedia(`(resolution: ${ratio}dppx)`);
    this.onMedia = () => {
      if (!this.active) return;
      this.#watchRatio();
      this.#sync();
    };
    this.media.addEventListener('change', this.onMedia, { once: true });
  }

  #unwatchRatio() {
    if (this.media && this.onMedia) {
      this.media.removeEventListener('change', this.onMedia);
    }
    this.media = null;
    this.onMedia = null;
  }
}

/**
 * Whether a frame `have` already answers a request for `want` at `draft`.
 *
 * Same size and resolution, and either already crisp or no crisper than what
 * is being asked for. `null` never covers anything.
 */
function covers(have, want, draft) {
  if (!have) return false;
  if (have.width !== want.width || have.height !== want.height || have.dpi !== want.dpi) {
    return false;
  }
  return !have.draft || draft;
}
