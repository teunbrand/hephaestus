# crates/hephaestus-explorer/CLAUDE.md

Windows Explorer shell extensions for `.hep` documents. One DLL, two
extensions:

- **`IThumbnailProvider`** — a real icon in Explorer rather than the generic
  document one. The counterpart of the macOS `QLThumbnailProvider` and the
  Linux `.thumbnailer`.
- **`IPreviewHandler`** — the Preview Pane (Alt+P), and **the only shell
  integration in this repo that can reflow**; see below.

The rendering is not here. Both call
`hephaestus-thumbnailer`'s library, which is shared with the Linux
thumbnailer — so aspect handling, the "solve at the document's natural size"
choice and the size clamping exist once.

## Read this first: none of it has run

**It type-checks against `x86_64-pc-windows-msvc` and has never been built,
registered or executed.** That target is installed here, so `cargo check` and
`cargo clippy --target x86_64-pc-windows-msvc` are real checks — they catch
wrong interface signatures, missing `windows` crate features and borrow
errors, which is a lot. They prove nothing about whether Explorer loads the
DLL.

Shell extensions fail the same way Quick Look extensions do: silently. Expect
to debug by bisection, and see "What to check first" below.

**`TESTING-WINDOWS.md` is the ordered script to hand to whoever has a Windows
machine.** It covers the app as well as these extensions, and it leads with the
one thing that is a *measurement* rather than a check: the IPC transport cost,
which the viewer's PNG-encoding default rests on and which nobody has
reproduced.

```sh
cargo check  --target x86_64-pc-windows-msvc     # works from any host
cargo clippy --target x86_64-pc-windows-msvc -- -D warnings
cargo build  --release --target x86_64-pc-windows-msvc   # needs a Windows host or MSVC libs
```

## The preview pane can reflow, and that is the interesting part

`IPreviewHandler` is given an `HWND` and told when its rectangle changes
(`SetRect`), so the plot is **re-solved at the new size** on every resize —
axes re-lay-out, ticks recompute, text re-wraps. That is what shipping a
document rather than a picture is for, and it is the thing the macOS
data-based preview API structurally cannot do (`QLFilePreviewRequest` carries
nothing but a URL; see `../hephaestus-quicklook/CLAUDE.md`).

Two consequences the code is shaped around:

- **The renderer is kept across resizes.** Acquiring a GPU adapter is well
  over 100 ms against a few milliseconds for the drawing, so `PreviewHandler`
  holds a `hephaestus_thumbnailer::Renderer` for the life of the preview
  rather than building one per paint.
- **Rendering happens on `SetRect`, not in `WM_PAINT`.** Painting is a
  `SetDIBitsToDevice` of the last render, which keeps the message loop cheap.
  There is deliberately **no debounce** yet: a fast drag will re-render per
  step, and whether that needs one is the first thing to measure on real
  hardware.

## Both handlers take a stream, not a path

`IInitializeWithStream` rather than `IPersistFile`, because that is what lets
the shell host the extension in its low-privilege surrogate — `dllhost.exe`
for the thumbnail, `prevhost.exe` for the preview — instead of loading it into
Explorer itself. A crash then costs a thumbnail or a preview rather than the
desktop. It is also what Microsoft documents as preferred.

The preview handler additionally sets `AppID` on its class to
`{534A1E02-D58F-44f0-B58B-36CBED287C7C}`, which is the 64-bit `prevhost.exe`
surrogate. Without it the handler is hosted in-process.

## Two conversions that fail quietly

Both are in `bitmap.rs`, and neither errors when wrong:

- **A `BI_RGB` 32-bit DIB is BGRA, not RGBA** — blue first. Getting it wrong
  swaps red and blue in every image, which on a plot reads as a subtly wrong
  palette rather than as breakage.
- **A DIB is bottom-up unless its height is negative.** `hephaestus` produces
  top-down rows like every other image API, so `top_down_info` passes a
  negative height. A positive one flips every image vertically.

Alpha is copied but the thumbnail reports `WTSAT_RGB`, so the shell ignores it.
A plot is rendered onto an opaque background, which makes that honest and also
sidesteps whether `WTSAT_ARGB` wants premultiplied alpha.

## Registration

`regsvr32 hephaestus_explorer.dll` calls `DllRegisterServer`, which writes:

| key | why |
| --- | --- |
| `HKCR\CLSID\{clsid}\InprocServer32` | where COM finds each class |
| …`\ThreadingModel` = `Apartment` | neither handler is thread-safe; state is behind a `RefCell` |
| `HKCR\CLSID\{preview}` value `AppID` | hosts the preview in `prevhost.exe` |
| `HKCR\.hep\ShellEx\{e357fccd-…}` | the thumbnail slot |
| `HKCR\.hep\ShellEx\{8895b1c6-…}` | the preview slot |
| `HKLM\…\CurrentVersion\PreviewHandlers` | the shell's list of preview handlers, read from HKLM only |

Registered against the **extension** key rather than a ProgID, so it does not
depend on how Tauri's NSIS installer happens to name the file association.

**This needs administrator rights** — both `HKEY_CLASSES_ROOT` (a merged view
that writes to `HKLM\Software\Classes`) and the HKLM list. A per-user
alternative exists for the class and slot keys, under
`HKCU\Software\Classes`, but the preview-handler *list* is not read from HKCU,
so a per-user install would get thumbnails and no preview pane. Not
implemented; noted so the trade is known.

The class ids in `lib.rs` are **permanent**. They are written into the
registry, so changing one orphans every installation that registered the old
value.

## What to check first, when it does not work

In this order, because each rules out everything below it:

1. **Does the DLL load at all?** `regsvr32` reports failure loudly; a
   *silent* success followed by nothing happening means the registration is
   right and the shell is rejecting the class.
2. **Is the file type recognized?** The thumbnail and preview slots hang off
   `.hep`, which Tauri's installer creates. Check `HKCR\.hep` exists.
3. **Restart the shell.** Explorer caches handlers aggressively:
   `taskkill /f /im explorer.exe && start explorer.exe`, and for thumbnails
   also clear the cache (Disk Cleanup → Thumbnails, or delete
   `%LocalAppData%\Microsoft\Windows\Explorer\thumbcache_*.db`).
4. **Watch the surrogate.** `dllhost.exe` / `prevhost.exe` appearing in Task
   Manager when a `.hep` is selected means the extension is being hosted and
   the problem is inside it.
5. **Prove the renderer works outside the surrogate first.** `cd
   ../hephaestus-thumbnailer && cargo run --release -- -s 512 -i some.hep -o
   t.png` uses the same rasterizer with no shell involved, which separates
   "the GPU works here" from "the GPU works inside Explorer's sandbox".
6. **Suspect the GPU.** This renders through `vello-hybrid`, so a surrogate
   with no D3D12 adapter produces nothing. That is the strongest argument for
   the `blend2d` CPU backend the parent crate has a placeholder for, and the
   alternative worth considering is `Windows.Data.Pdf` — Windows has a system
   PDF rasterizer, so the macOS trick of rendering PDF and letting the OS
   rasterize it would work here too and would need no GPU. Deliberately not
   taken: it adds WinRT async plumbing to code nobody can test yet, and fewer
   moving parts matters more than elegance while that is true.

## Not built yet

- **Anything verified.** See above, and `TESTING-WINDOWS.md`.
- **A debug channel that is on by default.** `--features diagnostics` routes
  progress through `OutputDebugStringW`, which DebugView can see from inside a
  surrogate — the only channel that reaches out of one. Off in a normal build,
  and the closure form means nothing is formatted when it is off.
- **A universal / ARM64 build.** `aarch64-pc-windows-msvc` would want its own
  DLL, and Explorer loads the one matching its own architecture.
- **Installer wiring.** `bundle.windows.nsis.installerHooks` in
  `../hephaestus-viewer/tauri.conf.json` is the place to call `regsvr32`, and
  `../hephaestus-viewer/packaging/installer-hooks.nsh` is written but unrun.
- **A debounce on resize**, if measurement says one is needed.
- **Authenticode signing.** Not required for a shell extension to load, but
  SmartScreen will complain about the installer without it. Unrelated to the
  macOS notarization thread.
- **A property handler** (`IPropertyStore`), which would put a plot's size or
  writer version in Explorer's detail columns. Small, and nobody has asked.

## Cross-references

- `../hephaestus-thumbnailer/CLAUDE.md` — the shared renderer, and the Linux
  integration that needed none of this.
- `../hephaestus-quicklook/CLAUDE.md` — the macOS equivalents, and why the
  preview there cannot reflow.
- `../hephaestus-viewer/CLAUDE.md` — the app, the file association, and the
  packaging scripts.
