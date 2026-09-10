# hephaestus-viewer

A desktop viewer for hephaestus plot documents. Double-click a `.hep` and it
opens; drop one on the window and it becomes a tab; resize and the plot
**reflows** — axes re-lay-out, ticks recompute, text re-wraps — rather than
stretching.

A `.hep` carries a plot's *configuration*, not a picture of it, which is what
makes that possible: the viewer re-solves the layout at whatever size the
window has. It is a Tauri shell whose webview is chrome and canvas only, over a
native Rust core that reads documents and rasterizes them on the GPU through
`vello-hybrid`. There is no WebAssembly involved.

## What it does

- **Several documents at once**, one per window. On macOS the windows carry a
  shared tabbing identifier, so AppKit groups them into a **real** tab bar —
  drag to reorder, drag out to detach, ⌘⇧[ and ], Merge All Windows. On Windows
  and Linux, which have no OS-level window tabs, they are separate windows.
  Opening a file that is already open brings its window forward rather than
  duplicating it.
- **Opens from anywhere** — the File menu, drag and drop, a command-line
  argument, a double-click in Finder or Explorer, or a second launch, which
  forwards its argument to the running window.
- **Watches and reloads.** Re-run whatever writes the `.hep` and the tab
  updates. A file caught mid-write is retried; a file this build genuinely
  cannot read leaves the last good plot on screen and says why.
- **Exports** to PNG, JPEG, TIFF, WebP, SVG and PDF, at a size in inches,
  millimeters, points or pixels. Raster files declare their resolution, so they
  claim the physical size they were rendered for. SVG comes out editable —
  real `<text>` elements — and PDF comes out with its fonts embedded.
- **Inverts the theme**, following the desktop's light or dark setting until
  told otherwise.

## Building

```sh
cargo run                                  # opens an empty window
cargo run -- path/to/plot.hep              # …with a document
cargo test                                 # no GPU needed
cargo test --test render_smoke             # needs a working wgpu adapter
node ui/verify.mjs                         # the frontend, and its seam with Rust

HEPHAESTUS_VIEWER_TRACE=1 cargo run --release -- plot.hep   # what each frame costs
```

The Tauri CLI is not needed for any of the above — the frontend is plain ES
modules with no build step. It *is* needed to produce an installable bundle,
which is also the only way to exercise the macOS double-click path:

```sh
cargo install tauri-cli --version '^2'
cargo tauri build
```

## Where things are

| | |
| --- | --- |
| `src/render/` | the thread that owns every document, the renderer, and the watcher |
| `src/windows.rs` | one window per document, and what makes them tab natively |
| `src/channel.rs` | the only seam between Tauri's threads and that one |
| `src/commands.rs` | what a window can ask for |
| `ui/` | the frontend: three ES modules, no bundler |
| `CLAUDE.md` | why it is shaped this way — start there before changing anything |

The short version of the shape: a `PlotComposition` is deliberately neither
`Send` nor `Sync`, so one thread owns all of them and everything else talks to
it over a channel — and one window shows one document, which is what earns the
native tab bar instead of a drawn one.
