# crates/hephaestus-viewer/CLAUDE.md

The desktop viewer: double-click a `.hep` and it opens, drop one on a window
and it opens beside it, resize and the plot **reflows** rather than scaling.
Plus the two things a viewer uniquely adds — export to every format the crate
writes, and watch-and-reload for iterating on a plot upstream.

Its own workspace, not a member of the crate above it, for the same reason the
two wasm clients are and arrived at from the opposite direction: cargo honors
`[profile]` only at a workspace root, and this one is an *application*. It
wants `opt-level = 3` / `lto = "thin"`, which is the last thing a wasm bundle
should inherit. See the note in `../../Cargo.toml`.

## Read this first: two facts the shape follows from

**One window shows one document**, for its whole life. See
[the window model](#one-window-per-document-which-is-what-buys-native-tabs)
below — it is the reason there is no tab strip in the HTML and no document id
on the command wire.

**One thread owns every document**, because a composition cannot leave it.

## One thread owns everything

**A `PlotComposition` is deliberately neither `Send` nor `Sync`** — its geoms
memoize shaped text behind `RefCell`/`Rc`. So it can go in no
`tauri::State<Mutex<…>>`, be guarded by no lock, and be moved to no other
thread once it exists.

Everything about this crate's shape follows from that:

- **One dedicated OS thread** (`render::spawn`) owns every open document, the
  single `HybridRenderer`, and the watcher. Nothing else touches a composition.
- **Tauri's managed state is two things**: `Render`, which is a
  `Mutex<mpsc::Sender<Request>>` and nothing more, and `PendingOpens`, which is
  a list of paths. `channel.rs` ends with `const _: () =
  assert_send::<Request>();` — adding a request field that holds a composition
  or a scene fails there, at compile time, rather than at the line that sends
  it.
- **Every command is `async`.** A plain `#[tauri::command]` runs on the main
  thread and all of these wait on the render thread, so a synchronous one would
  park the event loop for the length of a frame — the very thread a resize
  needs.

Two things fall out of it for free, and are worth not undoing: `hephaestus::text`'s
font collection is behind a global mutex and its layout context is
thread-local, so a single render thread contends with nobody and stays warm
across frames. Note that this is unaffected by there being many *windows* —
they all talk to the one thread, and share the one renderer.

## One window per document, which is what buys native tabs

The model is a document app's rather than a browser's. Opening a second
document opens a second window, and **on macOS every window carries the same
`tabbingIdentifier`, so AppKit groups them into a real tab bar** — drag to
reorder, drag out to detach, ⌘⇧[ and ], Show All Tabs, Merge All Windows. None
of that can be had from a tab strip drawn in a webview, and all of it is one
builder call (`windows::open_window`). The tab's label is the window's title,
which is why the title is set to the file name.

Consequences worth knowing before changing anything here:

- **No command takes a document id.** Rust reads it off the window the request
  came from (`WindowMap::tab_for`). A `tab` on the wire would be a second
  source of truth for something the window already is, and the two could
  disagree; `ui/verify.mjs` checks that none has crept back.
- **A window is bound to its document before it exists**, so a booting window
  *asks* what it is showing (`attach`) rather than being told. That removes the
  race the previous shape needed a `PendingOpens` buffer for: there is no
  longer a moment where a document has arrived and there is nobody to tell, so
  `RunEvent::Opened` needs no buffering at all.
- **The first window is created synchronously in `setup`, unconditionally.**
  Reading a document is not instant, and the event loop exits the moment it
  finds itself with no windows — so opening the first one from the async task
  that reads the arguments is enough to make the app start and immediately
  stop. It opens empty and is then *filled*, which is the same path File ▸ Open
  from a blank window takes, so the reuse is not a special case.
- **Closing a window closes the document.** There is no other way to reach one,
  so keeping it would leak a composition and a directory watch.
- **⌘W is the platform's Close Window**, not an item of ours: letting AppKit do
  it is what makes it behave correctly for a tab inside a group.
- **Menu items go to the focused window only.** A global emit would export
  every open plot on one ⌘E.

### Why the titlebar was left alone

The obvious native-feel move — an overlay titlebar with the toolbar's contents
beside the traffic lights — is **wrong once the tabs are native**, and would
have been a real bug. An overlay titlebar means `fullSizeContentView`, so the
webview's content extends *under* the titlebar region; the native tab bar lives
in that same region, so the toolbar would be hidden behind the tabs whenever a
window had more than one. The normal titlebar is also what gives the window a
real title and proxy icon, and the title is what labels the tab. So: standard
titlebar, native tab bar under it, our slim toolbar under that — which is
Finder's arrangement.

What the chrome does do for native feel is smaller and safer: `color-scheme:
light dark` so the platform draws its own scrollbars, controls and focus rings;
the `AccentColor` / `AccentColorText` system keywords so accents are the one
the user actually chose rather than a hardcoded orange; `-apple-system` with
`-webkit-font-smoothing: antialiased`, without which chrome text renders
visibly heavier than every other app on screen; the export modal as a sheet
dropping from the top edge with ⌘. to dismiss; `user-select: none`,
`overscroll-behavior: none`, and a suppressed context menu, since a webview's
Reload / Inspect Element menu is the single most obvious tell that a window is
a browser.

Vibrancy behind the toolbar is deliberately **not** done. It needs a
transparent window, which needs `macOSPrivateApi` and the `macos-private-api`
cargo feature, and the plot canvas is opaque and covers the window anyway — so
the cost is a private-API dependency for an effect visible in one 38 px strip.

## There is no wasm here

A Tauri app is a **native Rust binary** with a webview for chrome. This links
`hephaestus` by path, rasterizes through `vello-hybrid` on the real GPU, and
sends finished pixels over. Do not reach for `crates/hephaestus-wasm`: the two
share conventions, not code.

Because the webview owns presentation there is no surface to share and
`render_to_texture` is never called. `Renderer::render_to_buffer` normalizes to
**straight, un-premultiplied alpha on both backends** (see
`src/backend/mod.rs`), which is byte-for-byte what canvas `putImageData`
expects — so the premultiply divergence `src/window/CLAUDE.md` reasons about
simply never arises here.

`vello-hybrid` rather than `vello`, for the reason the wasm client picked the
same rasterizer: no draw-count ceiling, so a 100k-mark scatter renders at all.

## The frame wire format

A frame crosses to the webview as **one opaque buffer**: a 32-byte header, then
either raw RGBA8 or a PNG. `src/render/frame.rs` writes it and `ui/frame.js`
reads it; `tests/frame_header.rs` and `ui/verify.mjs` are what keep the two in
step, since nothing else would fail if they drifted.

Two decisions in it are worth knowing:

- **The outcome rides in the header, not in a `Result`.** A frame overtaken by
  a newer one for the same tab comes back `FrameStatus::Superseded` with no
  payload, and the frontend drops it silently. During a resize drag that is the
  *common* case, so reporting it as a rejected promise would mean an exception
  per pointer move at every layer it passed through.
- **The header is 32 bytes because that is a multiple of four.** `ImageData`
  refuses a `Uint8ClampedArray` whose offset into the buffer is not, and the
  frontend takes a view rather than a copy precisely to avoid moving eight
  megabytes twice.

`Vec<u8>` from a command becomes `InvokeResponseBody::Raw` and is served as
`application/octet-stream`, reaching JS as an `ArrayBuffer` — verified in the
Tauri source, not assumed. It is *not* JSON-encoded as an array of numbers.
`asBytes` in `ui/frame.js` handles the array form anyway, because the
difference would otherwise show up only as catastrophic slowness.

### The encoding seam, and the Windows number that justifies it

`Encoding::{RawRgba, PngFast}` exists because Tauri's binary IPC is not
equally fast everywhere: upstream measures a few milliseconds per ten megabytes
on macOS and around **200 ms per ten megabytes on Windows** (tauri#11915). So
Windows defaults to `PngFast` — a plot is flat color and compresses hard, and
`createImageBitmap` decodes it *off* the main thread, which raw pixels cannot
do.

`HEPHAESTUS_VIEWER_TRACE=1` reports every frame's size and cost on stderr, and
`HEPHAESTUS_VIEWER_ENCODING=raw|png` forces either encoding, which is what
makes the two comparable on one machine. Measured here, release build, macOS
arm64, `examples/document.hep` in a 1200×800 window on a 2× display —
**2400×1464 device pixels**:

| | bytes per frame | steady-state frame |
| --- | --- | --- |
| `RawRgba` | 14.05 MB | 10.6 ms |
| `PngFast` | 282 kB | 17.0 ms |

Two things to read off that. **The compression ratio is ~50×**, far better than
the "well under 1 MB" the design assumed, because a plot really is mostly flat
fills — and 282 kB is small enough that Windows' IPC cost stops being the
bottleneck, where 14 MB raw would be roughly 280 ms of transport per frame.
And **a frame is bigger than a naive estimate suggests**: the design reasoned
about 8 MB, and a 2× display in a modest window is already 14 MB.

The first frame in a fresh process is 130–220 ms rather than 10, which is
adapter and device acquisition, not rendering; it is paid once.

**The Windows and Linux figures are still upstream's, not measurements.**
Nobody working on this crate has had either platform in front of them.
Reproducing the table above on both is the first thing to do with access, and
the two env vars exist for exactly that.

### Draft frames

`FrameSpec::draft` renders at half the size *and* half the dpi. Halving both
leaves the point-space extent — `px / dpi × 72`, the only thing the layout is
solved against — unchanged to within half a pixel, so a draft is the same
picture at a quarter of the pixels rather than a different one. That claim is
load-bearing for the whole resize feel, so
`tests/render_smoke.rs::a_draft_frame_solves_the_same_layout` measures where
the ink actually lands in both and holds them to 2%.

The frontend asks for a draft while the size is moving and a crisp frame 150 ms
after it settles. The *first* frame of a document is never a draft: there is no
previous picture to stretch, so a draft would only be briefly blurry.

## The resize loop is the wasm client's, inverted

Sizes are **device pixels** and dpi is **`96 × devicePixelRatio`**, which is
the pair that makes a theme length in points come out the right physical size —
same as `crates/hephaestus-wasm/js/hephaestus.js`. The `ResizeObserver` asks
for `device-pixel-content-box` and falls back to the CSS box times the ratio,
and a re-armed `matchMedia('(resolution: …dppx)')` catches a move between
monitors of different densities, which changes the ratio without necessarily
changing the CSS box.

What is **not** shared is when the drawing happens, and the difference is the
one thing to understand before touching `ui/view.js`:

| | wasm client | here |
| --- | --- | --- |
| pixels available | synchronously, in the observer callback | asynchronously, from another thread |
| so | assign `canvas.width`, draw, same paint | canvas is sized in **CSS only** |
| backing store assigned | in the callback | in the same synchronous block as `putImageData`, when pixels arrive |
| during a drag | every frame is crisp | the previous frame stretches until the new one lands |

Assigning `canvas.width` **clears the drawing buffer**. The wasm client gets
away with doing it early because it draws before the browser next paints; here
it cannot, so the buffer is left alone until there are pixels for it. The
canvas is therefore never cleared and never sized to something it has no
picture for. `frame.js::resizeTo` is the whole of it, and the PNG path decodes
*before* resizing for the same reason.

One outstanding frame per view, latest wins — and a pending crisp request is
never downgraded to a draft by a later one, or letting go of the mouse could
leave a blurry picture on screen. The render thread coalesces on its own side
too (`Worker::dispatch`), so a queue that built up while a frame was drawing
costs one render rather than ten.

## Opening a file: four paths, and only three are testable unbundled

- **Dialog** — `tauri-plugin-dialog`, **from Rust**. A Rust-side dialog needs no
  entry in `capabilities/default.json`, and the frontend has no bundler, so the
  alternative was hand-rolling `invoke('plugin:dialog|…')` against a permission
  list. The callback form is used rather than `blocking_*`, so nothing blocks
  an async task.
- **Drag-drop** — Tauri's `dragDropEnabled` is on, and it **suppresses HTML5
  drag and drop in the webview**. A `drop` listener would never fire;
  `getCurrentWebview().onDragDropEvent()` is the only route.
- **argv** — Windows and Linux associations pass the path as an argument. Note
  the filter on arguments starting with `-`: macOS passes `-psn_…` to a bundled
  app.
- **macOS double-click** — Launch Services sends an Apple Event, not an
  argument. Tauri surfaces it as `RunEvent::Opened { urls }`. **Only reachable
  in a bundled build** (`cargo tauri build`), never from `cargo run`, and see
  below for the ordering trap that makes it the hardest of the four.

### `RunEvent::Opened` fires before `setup`

Not "can fire before the webview loads" — **before the setup hook itself**.
Measured on a first launch by double-click, with a timestamped log:

```
141.864  RunEvent::Opened ["/private/tmp/dbl.hep"]
141.874  setup: argv paths = []          ← 10 ms later
```

Which matters because `Render` is managed *in* `setup`. Reaching for it on the
open path with `Manager::state` — the panicking accessor — panics inside the
spawned task that would have opened the document, and **a panic in a spawned
task is swallowed**. The symptom is an app that comes up empty on the first
double-click and works on the second, with nothing logged anywhere.

So `deliver` uses `try_state` and, when the render thread is not up yet, holds
the paths in `PendingOpens` (`windows.rs`); the end of `setup` drains them
after starting the thread and opening the first window. Arguments are
delivered by the same call, so there is one path rather than two.

### Observing a launch that Launch Services started

The double-click path can only be exercised by letting Launch Services start
the process, which means no control over the environment — so for a while the
only way to see anything was temporary instrumentation writing to a file.
`open` can do better, and this is the way to check it:

```sh
open --env HEPHAESTUS_VIEWER_TRACE=1 --stderr /tmp/launch.log \
     -a "…/Hephaestus Viewer.app" some.hep
cat /tmp/launch.log        # a frame line means the document loaded
```

**Run the no-file control alongside it**, or the check proves nothing: launched
with no argument the app must render *nothing* (an empty window asks for no
frames), so a frame in the first case is evidence the document arrived rather
than evidence the app started.

Note that `open` on a file whose handler is already running is the *second*
double-click, which always worked — `pkill -f "Hephaestus Viewer.app"` between
runs or the interesting case is skipped entirely.

`ui/verify.mjs` checks that `deliver` still uses `try_state`, still buffers,
and that something still drains — each of which alone would silently
reintroduce the bug.

A note on why the *previous* attempt at this was wrong. An earlier shape
buffered until the frontend collected, on the theory that the risk was a window
not yet listening. That was deleted when the window-per-document model made a
binding exist before its window did — which is true, and does close that race.
The race it does not close is this one: managed state not existing yet. Two
different problems with similar-looking symptoms.

`tauri-plugin-single-instance` is registered **first**, as it requires, and
forwards a second launch's argv to the running process. Needed on Windows and
Linux; macOS already single-instances a bundled app.

A path is canonicalized on open, and that canonical form is the document's
identity: it is what recognizes a re-open through a different spelling (so
double-clicking the same file twice focuses its tab rather than opening a
second copy) and what the watcher's event paths can be compared against.

### The one hand-written piece of the association story

`bundle.fileAssociations` gets macOS `CFBundleDocumentTypes` and the Windows
`Software\Classes` keys for free, and writes the Linux `.desktop` entry's
`MimeType=`. What nothing generates is a declaration of what
`application/x-hephaestus-plot` *is* — so `packaging/hephaestus-viewer.xml` is
a shared-mime-info file with a `<magic>` rule on the container's ASCII
`HEPHPLOT` at offset 0, installed by the deb and rpm `files` maps. Without it
the desktop has a handler for a type it cannot recognize. **The exact packaging
hook is unverified** — nobody has run `cargo tauri build` on Linux here.

## Watching, and why the watch is on the directory

**The watch goes on the parent directory, non-recursively — never on the
file.** A careful writer saves atomically: write a temporary file, rename it
over the target. That replaces the inode, so a watch registered against the
file follows the old one, survives exactly one save, and then goes permanently
deaf *without reporting anything*. One directory is watched once however many
documents in it are open, hence the reference counting in `watch.rs`.

An event path is not guaranteed to be spelled the way the document was
registered — on macOS an FSEvents path arrives resolved through `/private`, so
`/tmp/a.hep` is reported at `/private/tmp/a.hep`. `Registry::tab_for` tries the
literal path and then the canonical one.

A removal is deliberately **not** worth reloading. A document whose file has
gone is still a plot on screen that can be exported, so the viewer keeps
showing it rather than reporting a failure nobody asked about — and a rename
over the file, which is how an atomic save arrives, is reported as a
modification of the destination rather than a removal.

### Retryable versus fatal

A watcher can fire while a writer is halfway through, and a truncated prefix
fails to decode in whatever way the cut happened to land. `error::retryable`
splits the two cases because both are common and the right answer is opposite:

- **Fatal** — `UnsupportedVersion` (the reader's check is **equality** on
  `FORMAT_VERSION_MAJOR`, not a floor, so one end or the other needs a
  different build), `UnsupportedFlags`, `UnknownCriticalChunk`, `UnknownGeom`.
  Statements about the document; waiting will not help.
- **Everything else** — retried at 150 / 400 / 900 ms. Written as a wildcard
  match so a new `DocumentError` variant defaults to retryable, which is the
  safe direction: a spurious retry costs 150 ms, a spurious banner costs
  trust.

`tests/reload_classify.rs` checks the classification *and* checks that a real
truncated document lands on the retry side — every seventh prefix of the
fixture, since where the cut falls decides which error comes out.

**A failed reload keeps the last good composition on screen.** The file on disk
is broken; the plot the user is looking at is not, and it is still exportable.
The tab is marked and a banner explains.

## Export

Every format starts from a composition **read back from the document's own
bytes** (`OpenDoc::fresh_composition`), never from the one on screen. Drawing
leaves a solved layout cached against the size it was drawn at, and an export
is almost never that size. `examples/document_load.rs` in the parent crate does
the same thing for the same reason.

- **Raster** — device pixels at the requested dpi, one `render_to_buffer`, then
  the writer in `src/image/`. The dpi is passed through to every writer: a file
  that declares nothing is read as 72 dpi, so a 300 dpi export would claim four
  times its physical size. `tests/render_smoke.rs` reads the `pHYs` chunk back
  to check it.
- **SVG and PDF** — both are handed **`dpi = 72` and a size in points**. Then a
  length in the theme, a coordinate in the file and the page's own size are all
  one unit: for PDF the size *is* the MediaBox, and for SVG `SvgUnits::Pt`
  writes a physical size whose numbers match the `viewBox` rather than needing
  conversion into it. Handing SVG 96 dpi and asking for `Pt` would emit a
  width in points carrying a pixel count, which is wrong by 1.333.
- **The size check comes before the renderer check.** A 120000 px request is
  refused as a size problem, not as a missing adapter — the second message is
  true and names the wrong cause. `tests/export_vector.rs` pins the ordering,
  which is where the bug was found.
- **A machine with no adapter can still export SVG and PDF.** The renderer is
  built lazily and its failure remembered, so it is asked for once rather than
  once per frame, and `export()` takes `Option<&mut HybridRenderer>`.

`scene.warnings()` from both vector backends reaches `ExportReport::warnings`.
That is the crate's degradation channel — a flattened sweep gradient, a font
that could not be embedded — and dropping it would make those silent.

## Building and checking

The **Tauri CLI is not needed for `cargo build`**: the frontend is plain ES
modules with no bundler, so `frontendDist` is a directory that already exists
and there is no `beforeBuildCommand`. It *is* needed to produce a bundle, and
that is the only way to test the macOS double-click path.

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings
cargo test                      # header, reload classifier, SVG/PDF export — no GPU
cargo test --test render_smoke   # frames and raster export — needs a wgpu adapter
node ui/verify.mjs               # the frontend, and its seam with Rust
cargo run -- ../../examples/document.hep   # unbundled, argv path
cargo tauri build                # then test double-click and drag-drop on the bundle

# What each frame costs, and which encoding carried it.
HEPHAESTUS_VIEWER_TRACE=1 cargo run --release -- ../../examples/document.hep
HEPHAESTUS_VIEWER_TRACE=1 HEPHAESTUS_VIEWER_ENCODING=png cargo run --release -- …
```

A `cargo build` of any profile is a Tauri **dev** build (`cargo:rustc-cfg=dev`
from the build script), which is enough to run the app and exercise every open
path but argv is the only one reachable without a bundle. An edit under `ui/`
does trigger a rebuild and does reach the binary — checked, since the frontend
is embedded rather than read from disk.

`Cargo.lock` **is committed** here, unlike in the two wasm siblings: this is an
application, and a reproducible build of one is worth having.

**A second launch is silent, which is confusing while developing.**
`tauri-plugin-single-instance` is doing its job: it forwards the new process's
arguments to the running one and exits `0` with no output. So a stray instance
left over from an earlier run makes every subsequent `cargo run` look like an
app that starts and immediately dies for no reason, with nothing on stderr to
say why. `pgrep -f hephaestus-viewer` first; it cost an hour once.

### Packaging, signing, and the Quick Look extension

`cargo tauri build` produces the app; it does **not** produce the app that
Finder previews a `.hep` with, because Tauri's bundler has no concept of an app
extension. `./package-macos.sh` drives the bundler and then does the three
things it cannot — build the two appexes (a space-bar preview and a Finder
thumbnail, separate bundles because an extension declares one extension
point), put them in `Contents/PlugIns/`, and re-sign:

```sh
./package-macos.sh                                    # ad-hoc, this machine
./package-macos.sh "Apple Development: …"             # a real identity
./package-macos.sh "Developer ID Application: …"      # for distribution
```

Three things about it are load-bearing:

- **Signing is inside-out.** A bundle's signature covers its nested code, so
  the appex is sealed before the app is signed over it. `--deep` is not the
  answer and is deprecated — it would re-sign the appex with the *app's*
  entitlements, dropping the App Sandbox a Quick Look extension must have.
- **The app is deliberately not sandboxed** (see `macos/entitlements.plist`).
  It opens documents from anywhere, watches their parent directories, and
  writes exports wherever a save dialog says; the directory watching in
  particular has no clean sandboxed answer. The extension inside it *is*
  sandboxed, which is mandatory there and unrelated.
- **`disable-library-validation` is not optional.** wgpu loads Metal and the
  system's driver bundles, which this app did not sign. Without the exception
  the hardened runtime refuses them and no adapter is found — surfacing as
  "cannot rasterize", not as a signing error.

The UTI is what ties the two together, and Tauri does emit it:
`bundle.fileAssociations[].exportedType` becomes `UTExportedTypeDeclarations`,
so `dev.posit.hephaestus.plot` is *declared* rather than merely claimed. The
extensions' `QLSupportedContentTypes` name the same string. Check the system
agrees with `mdls -name kMDItemContentType some.hep`.

**It conforms to `public.image`, not `public.data`**, which is what makes
Finder group a `.hep` with pictures rather than with generic documents —
`public.image` is itself `['public.data', 'public.content']`, so this is a
superset of the obvious choice rather than a swap.

Two things make that defensible rather than a lie. `public.svg-image` sets the
precedent: a vector description of one picture conforms to `public.image` even
though it is markup rather than samples, and a `.hep` is the same shape of
thing. And the feared side effect does not happen — **no image app starts
claiming the type.** Measured with `NSWorkspace.urlsForApplications(toOpen:)`,
which offers only this app, because Preview enumerates 54 *specific* content
types rather than declaring `public.image` wholesale, and Photos does the same.
Quick Look still routes to our own extension, since it matches the exact UTI.

The residual risk is a third-party app that *does* declare `public.image`
broadly: it would offer to open a `.hep` and fail. That is mild and contained,
and it is the whole cost of the grouping.

**Distribution is not done.** Only a Developer ID Application certificate can
sign for it, notarization is a separate step
(`xcrun notarytool submit` then `xcrun stapler staple`), and neither is
scripted. Local use works with an Apple Development identity.

See `../hephaestus-quicklook/CLAUDE.md` for the extensions themselves —
including the two `Info.plist` keys that are required and undocumented, the
coordinate-space trap in a thumbnail reply, and why `qlmanage` verifies
neither on macOS 26.

### `ui/verify.mjs` is not optional

Node has no DOM, so nothing can exercise a canvas here — which is precisely why
the checks are shaped the way they are. The failures worth catching are the
*silent* ones, and none of them is a compile error on either side:

- a `getElementById` whose id was renamed in the HTML — returns `null`, then a
  `TypeError` at boot and a window that does nothing;
- an `invoke` naming a command Rust no longer registers, or a command nothing
  calls (dead surface a `pub fn` in a lib target draws no warning for);
- a `listen` on an event Rust never emits, or an emitted event nobody hears;
- a command that has grown a document id back, when the window is the document;
- the window capability's label pattern drifting from the labels
  `windows::open_window` actually generates — which would deny every command
  and leave a window that silently does nothing;
- the macOS tabbing identifier going missing, which turns every document into a
  detached window with no tab bar;
- the export dialog offering a format `ExportFormat` has no variant for;
- the dialog's pixel ceiling drifting from the crate's own
  `MAX_TEXTURE_DIMENSION`.

Same bargain as `crates/hephaestus-wasm/verify-dist.mjs`: a crude static check
against finding out from a user. It was verified to *fail* on each of those,
which is the only thing that makes a passing run mean anything.

### The icons are generated, and crude

`icons/` was produced by a script, not drawn: a rounded slate square with three
bars, which reads at 16 px. `.icns` and `.ico` are assembled by embedding the
PNGs, which both formats have allowed for many years. `generate_context!`
**opens the first PNG in `bundle.icon` at compile time**, so a missing icon is
a build failure rather than a bundling one. Replace them with `cargo tauri
icon <source.png>`, which regenerates the whole set.

## Not built yet

- **A theme file format** (a crate-level want, broader than the viewer):
  something that can be parsed, applied, and extracted back out of a plot. It
  is the missing half of the theme editor below — an editor with nowhere to
  save to is a toy — and it belongs in the parent crate beside `document/`
  rather than here.
- **Theme editor with click-to-edit.** The plumbing exists: build the renderer
  `with_picking` and a click becomes `hits_at(x, y)`, whose scope chain is a
  typed `PlotPath` (`composition → plot? → region → axis|legend|geom → part →
  item`). Chrome is recorded through `ScopeMode::Target`, so a title and an
  axis label are hittable today. The work is mapping a `PlotPart` to the theme
  fields that style it, plus the editor UI; re-rendering after an edit is just
  another frame. Picking costs CPU per draw (~7 ms per 100k marks) whether or
  not anything is queried, so it should be opt-in per tab rather than always
  on. Persisting an edit needs the theme format above, or `document-write` and
  `write_composition` — at which point the viewer quietly becomes a light
  editor, so scope it deliberately.
- **Hover tooltips and zoom/pan.** Tooltips want `with_picking` on the render
  thread and a `pick_at` command; the index answers from the frame on screen,
  so there is nothing to read back.
- **A rasterizing path that skips the IPC.** A wgpu surface on the Tauri window
  behind a transparent webview would avoid the pixel transport entirely
  (`HybridRenderer::with_device` plus `set_target_format`; the sparse-strip
  backend targets a swap chain directly). Known-flickery upstream
  (tauri#9220), and the encoding seam plus draft frames were the cheaper
  answer. Noted so nobody rediscovers it as a first idea.
- **A tab strip for Windows and Linux.** Neither has an OS-level notion of
  window tabs, so there a second document is simply a second window. That is a
  legitimate desktop idiom and it is what ships, but Explorer, Terminal and
  Notepad all draw their own tabs now, so a drawn strip is the eventual answer
  *there* — while macOS keeps the native one. What makes this tractable is that
  it is purely additive: the window is already the unit, so a strip would be a
  way of switching between windows rather than a change to the model.
- **A Window menu listing open documents.** AppKit adds its own tab items to
  the Window menu, but not a document list, and Tauri's menu has to be rebuilt
  to change — so there is no cross-platform way to switch documents from the
  keyboard outside macOS's ⌘⇧[ and ].
- **Staying alive with no windows.** Closing the last window exits, which is
  right on Windows and Linux and not quite right on macOS, where a document app
  normally keeps its menu bar. Preventing the exit risks an app that cannot be
  quit if `ExitRequested` cannot distinguish the two causes, which is worth
  checking before doing it.
- **Per-window dark mode.** Inversion is app-wide — `attach` reports it so a
  new window adopts the mode rather than deciding again from the desktop and
  broadcasting a redraw to every other window. Per-document would mean the
  toggle stops describing what is on screen.
- **Supplying an image or font a document only names.** Both resolve against
  the machine, which on a desktop is the right default and is why
  `google-fonts` is deliberately absent — but a document naming a file that is
  not there draws those rows as nothing, with no way for the viewer to fill the
  gap.

## Cross-references

- `../../src/CLAUDE.md` — the `!Send` note this crate's whole shape follows
  from, and the API-level split.
- `../../src/document/CLAUDE.md` — what a document carries, and what a format
  major bump means.
- `../../src/backend/mod.rs` — the straight-alpha contract on
  `render_to_buffer`, and `MAX_TEXTURE_DIMENSION`.
- `../../src/window/CLAUDE.md` — the *other* host over the same backends, for
  contrast: it owns a surface, this one does not.
- `../hephaestus-wasm/CLAUDE.md` — the resize and light/dark conventions this
  crate mirrors, and the font trap that does not apply here.
