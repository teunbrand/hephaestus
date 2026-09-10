// Boot and wiring, for one window showing one document.
//
// Everything that is a policy decision rather than a picture lives here: what
// opening means, what the menu items do, and what the export dialog asks for.
//
// There is no tab strip and no notion of a document other than *this* one.
// Opening a second file opens a second window — which on macOS AppKit groups
// into a real tab bar, since every window carries the same tabbing identifier.
// So the code that used to switch between canvases is gone rather than hidden:
// a window's document is fixed for its whole life, and Rust reads it off the
// window rather than taking it as an argument.

import { PlotView } from './view.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWebview } = window.__TAURI__.webview;

/** Largest side a rasterized export can have, from the crate's own limit. */
const MAX_EXPORT_PX = 16384;

const dom = {
  plot: document.getElementById('plot'),
  empty: document.getElementById('empty'),
  banner: document.getElementById('banner'),
  bannerText: document.getElementById('banner-text'),
  bannerClose: document.getElementById('banner-close'),
  drop: document.getElementById('drop'),
  toast: document.getElementById('toast'),
  open: document.getElementById('open'),
  openEmpty: document.getElementById('open-empty'),
  exportButton: document.getElementById('export'),
  dark: document.getElementById('dark'),
  dialog: document.getElementById('export-dialog'),
  form: document.getElementById('export-form'),
  format: document.getElementById('format'),
  width: document.getElementById('width'),
  height: document.getElementById('height'),
  unit: document.getElementById('unit'),
  dpi: document.getElementById('dpi'),
  dpiField: document.getElementById('dpi-field'),
  quality: document.getElementById('quality'),
  qualityField: document.getElementById('quality-field'),
  svgFields: document.getElementById('svg-fields'),
  outlineText: document.getElementById('outline-text'),
  embedFonts: document.getElementById('embed-fonts'),
  note: document.getElementById('export-note'),
  cancel: document.getElementById('export-cancel'),
};

/** What this window shows, or `null` while it shows nothing. */
let current = null;
let dark = false;
let toastTimer = null;

const view = new PlotView(dom.plot, { onError: (text) => warn(text) });

// ─── The document ───────────────────────────────────────────────────────────

/** Show `info` in this window, or nothing when it is `null`. */
function show(info) {
  current = info;
  dom.empty.hidden = info !== null;
  dom.plot.hidden = info === null;
  dom.exportButton.disabled = info === null;

  if (info === null) {
    view.deactivate();
    hideBanner();
    return;
  }
  if (info.stale) warn(`This document could not be reloaded.\n${info.stale}`);
  else hideBanner();
  view.invalidate();
  view.activate();
}

/**
 * Report whatever an open request could not do.
 *
 * The documents that *did* open become windows of their own, arranged by Rust,
 * so there is nothing for this window to do with them.
 */
function reportFailures(outcome) {
  if (!outcome || outcome.failed.length === 0) return;
  warn(outcome.failed.map((failure) => `${failure.path}\n${failure.message}`).join('\n\n'));
}

async function openDialog() {
  try {
    reportFailures(await invoke('open_dialog'));
  } catch (error) {
    warn(String(error));
  }
}

async function openPaths(paths) {
  if (paths.length === 0) return;
  try {
    reportFailures(await invoke('open_paths', { paths }));
  } catch (error) {
    warn(String(error));
  }
}

// ─── Theme ──────────────────────────────────────────────────────────────────

async function setDark(next) {
  dark = next;
  dom.dark.setAttribute('aria-pressed', String(dark));
  try {
    await invoke('set_dark', { dark });
  } catch (error) {
    warn(String(error));
  }
}

// ─── Reporting ──────────────────────────────────────────────────────────────

function warn(text) {
  dom.bannerText.textContent = text;
  dom.banner.hidden = false;
}

function hideBanner() {
  dom.banner.hidden = true;
  dom.bannerText.textContent = '';
}

function toast(text) {
  dom.toast.textContent = text;
  dom.toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    dom.toast.hidden = true;
  }, 4000);
}

// ─── Export ─────────────────────────────────────────────────────────────────

/** Fill the dialog from this document and show it. */
function showExport() {
  if (current === null) return;

  // The writer's own size is the best default there is: it is what the plot
  // was composed for. Hints are in points.
  if (current.hint_width && current.hint_height) {
    dom.unit.value = 'in';
    dom.width.value = (current.hint_width / 72).toFixed(2);
    dom.height.value = (current.hint_height / 72).toFixed(2);
  }
  syncExportFields();
  dom.dialog.showModal();
}

/** Show only the options the chosen format has, and say what it will produce. */
function syncExportFields() {
  const format = dom.format.value;
  const raster = ['png', 'jpeg', 'tiff', 'webp'].includes(format);
  const pixelUnit = dom.unit.value === 'px';

  dom.qualityField.hidden = format !== 'jpeg';
  dom.svgFields.hidden = format !== 'svg';
  // A vector file has no resolution of its own, so dpi is only asked for when
  // it is what turns a pixel count into a size.
  dom.dpiField.hidden = !raster && !pixelUnit;

  const dpi = Number(dom.dpi.value) || 96;
  const width = Number(dom.width.value);
  const height = Number(dom.height.value);
  const perInch = { in: 1, mm: 1 / 25.4, pt: 1 / 72, px: 1 / dpi }[dom.unit.value];

  if (!(width > 0 && height > 0 && perInch > 0)) {
    dom.note.textContent = 'Width and height must both be positive.';
    return;
  }

  const inchesWide = width * perInch;
  const inchesHigh = height * perInch;
  if (raster) {
    const px = Math.round(inchesWide * dpi);
    const py = Math.round(inchesHigh * dpi);
    dom.note.textContent =
      px > MAX_EXPORT_PX || py > MAX_EXPORT_PX
        ? `${px} × ${py} px is past the ${MAX_EXPORT_PX} px limit — reduce the size or the resolution.`
        : `${px} × ${py} pixels, ${inchesWide.toFixed(2)} × ${inchesHigh.toFixed(2)} in.`;
  } else {
    dom.note.textContent = `${(inchesWide * 72).toFixed(0)} × ${(inchesHigh * 72).toFixed(
      0,
    )} pt, ${inchesWide.toFixed(2)} × ${inchesHigh.toFixed(2)} in.`;
  }
}

async function runExport() {
  if (current === null) return;
  const spec = {
    format: dom.format.value,
    width: Number(dom.width.value),
    height: Number(dom.height.value),
    unit: dom.unit.value,
    dpi: Number(dom.dpi.value) || 96,
    jpeg_quality: Number(dom.quality.value) || 90,
    outline_text: dom.outlineText.checked,
    embed_fonts: dom.embedFonts.checked,
  };
  try {
    const report = await invoke('export_document', {
      spec,
      suggestedName: current.title,
    });
    const size = `${(report.bytes / 1024).toFixed(0)} kB`;
    const warnings =
      report.warnings.length > 0
        ? `\n${report.warnings.length} warning(s): ${report.warnings.join(', ')}`
        : '';
    toast(`Wrote ${report.path} — ${report.width} × ${report.height}, ${size}.${warnings}`);
  } catch (error) {
    // Closing the dialog is not a failure worth reporting.
    if (String(error).includes('canceled')) return;
    warn(String(error));
  }
}

// ─── Wiring ─────────────────────────────────────────────────────────────────

dom.open.addEventListener('click', openDialog);
dom.openEmpty.addEventListener('click', openDialog);
dom.exportButton.addEventListener('click', showExport);
dom.dark.addEventListener('click', () => setDark(!dark));
dom.bannerClose.addEventListener('click', hideBanner);

dom.cancel.addEventListener('click', () => dom.dialog.close());
dom.form.addEventListener('submit', (event) => {
  event.preventDefault();
  dom.dialog.close();
  runExport();
});
for (const element of [dom.format, dom.unit, dom.width, dom.height, dom.dpi]) {
  element.addEventListener('input', syncExportFields);
  element.addEventListener('change', syncExportFields);
}

/** Menu items and their keyboard equivalents reach the same actions. */
function act(id) {
  switch (id) {
    case 'open':
      openDialog();
      break;
    case 'export':
      showExport();
      break;
    case 'toggle-dark':
      setDark(!dark);
      break;
    case 'reload':
      view.refresh();
      break;
    default:
      break;
  }
}

// Sent to the focused window only, so ⌘E exports the plot being looked at
// rather than every open one.
listen('menu-action', (event) => act(event.payload));

// Inversion is window-wide, so the window that was clicked is not the only one
// that has to redraw.
listen('theme-changed', (event) => {
  dark = Boolean(event.payload);
  dom.dark.setAttribute('aria-pressed', String(dark));
  view.refresh();
});

// This window was empty and has been given a document.
listen('document-bound', (event) => show(event.payload));

listen('document-reloaded', (event) => {
  if (current === null || event.payload.id !== current.id) return;
  current = event.payload;
  hideBanner();
  view.refresh();
});

listen('document-reload-failed', (event) => {
  const { id, message: text } = event.payload;
  if (current === null || id !== current.id) return;
  current = { ...current, stale: text };
  warn(`This document could not be reloaded.\n${text}`);
});

// Tauri's own drag-drop is enabled, which *suppresses* HTML5 drag and drop in
// the webview — so a `drop` listener would never fire and this is the only
// route. The filter is on the extension because a folder or a stray file
// dropped on a viewer should be ignored, not reported.
getCurrentWebview().onDragDropEvent((event) => {
  const { type, paths } = event.payload;
  if (type === 'enter' || type === 'over') {
    dom.drop.hidden = false;
    return;
  }
  dom.drop.hidden = true;
  if (type === 'drop') {
    openPaths((paths ?? []).filter((path) => path.toLowerCase().endsWith('.hep')));
  }
});

// The webview's own context menu — Reload, Inspect Element — is the single
// most obvious tell that a window is a browser. There is nothing selectable
// here for a menu to act on anyway.
window.addEventListener('contextmenu', (event) => event.preventDefault());

// ⌘. dismisses a sheet on macOS, which `<dialog>` does not do for itself.
window.addEventListener('keydown', (event) => {
  if (event.key === '.' && (event.metaKey || event.ctrlKey) && dom.dialog.open) {
    event.preventDefault();
    dom.dialog.close();
  }
});

// ─── Boot ───────────────────────────────────────────────────────────────────

async function boot() {
  let attachment;
  try {
    // A window created for a document is bound to it before it exists, so
    // this is an answer rather than a race.
    attachment = await invoke('attach');
  } catch (error) {
    warn(String(error));
    show(null);
    return;
  }

  dark = attachment.dark;
  dom.dark.setAttribute('aria-pressed', String(dark));

  // The plot's theme starts where the desktop is, and stops following it the
  // moment somebody says otherwise: a document's own light or dark is a
  // property of the picture, not of the desktop. Only the *first* window
  // decides — a later one adopts what the app is already in, which is why the
  // request is skipped when it would change nothing.
  const desktopDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  if (desktopDark !== dark) {
    await setDark(desktopDark);
  }

  show(attachment.document ?? null);
}

boot();
