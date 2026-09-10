# crates/hephaestus-thumbnailer/CLAUDE.md

Renders a `.hep` to a thumbnail image. A library and a binary:

- **The binary** is the **freedesktop thumbnailer** — the Linux file-manager
  integration.
- **The library** is the same rendering without a process boundary, which is
  what a Windows `IThumbnailProvider` needs, since that is a COM DLL loaded
  into Explorer's surrogate and cannot shell out.

Its own workspace, like every crate under `crates/`. See the note in
`../../Cargo.toml`.

## Linux integration is a data file, and that is the whole story

This is by far the cheapest of the three OS shell integrations, and the reason
is worth stating plainly: **there is no plugin ABI.** A file in
`/usr/share/thumbnailers/` names a command line, and the file manager runs it.

```
[Thumbnailer Entry]
TryExec=hephaestus-thumbnailer
Exec=hephaestus-thumbnailer -s %s -i %i -o %o
MimeType=application/x-hephaestus-plot;
```

`%s` is the largest edge the thumbnail may have, `%i` the input, `%o` where to
write a PNG. No COM, no registry, no sandbox, no code signing, no undocumented
`Info.plist` keys — compare `../hephaestus-quicklook/CLAUDE.md`, where three of
the four things that make an extension load fail silently.

It also means the whole thing is testable *anywhere*, including on macOS: the
binary is a plain CLI, so `cargo test` covers it and the output is a PNG to
look at. That is why this was built first.

**Who reads it.** GNOME Files (through gnome-desktop's thumbnail factory) and
Thunar (through tumbler). **Not KDE's Dolphin**, which uses its own C++ KIO
thumbnail plugins — a `.hep` gets no thumbnail there, and closing that would
mean a separate C++ shared library. Deliberately not done.

**The MIME type has to be recognized first.** The `.thumbnailer` matches on
`application/x-hephaestus-plot`, which
`../hephaestus-viewer/packaging/hephaestus-viewer.xml` declares — including a
`<magic>` rule on the container's `HEPHPLOT` bytes, so a renamed file still
gets a thumbnail. Without that XML installed the desktop has a thumbnailer for
a type it does not know.

## What the binary promises

- **`%s` is a maximum, not a target.** The thumbnail carries the document's own
  aspect, so one edge reaches `%s` and the other is smaller. A file manager
  expects exactly this; filling a square would distort a wide plot.
- **Non-zero exit and nothing written on failure.** That is what tells the
  factory to fall back to a generic icon rather than cache an empty file. An
  unreadable document, a missing file and a nonsense size all take that path.
- **`file://` URIs are accepted as well as paths**, with percent-decoding,
  because the spec offers both `%i` and `%u` and a `.thumbnailer` may be edited
  to use either.

## The layout is solved at the document's natural size

The plot is laid out at its own size hint (or 600×450 pt when it recorded none)
and *rasterized* to fit the requested box — not re-solved for a 128-pixel
square. Same choice as the macOS preview, same reason: the natural size is what
the plot was composed for, and a thumbnail should be a picture of the plot
rather than the plot laid out for a postage stamp. `dpi` follows from the
scale, so a thumbnail is a sharper or coarser rendering of one geometry.

## One adapter per file, which is the argument for a CPU backend

The freedesktop contract is one process per thumbnail, and this build
rasterizes through `vello-hybrid` — so **every thumbnail pays a GPU adapter
initialization.** Measured here: ~190 ms at 512 px, ~120 ms at 128 px, most of
which is that setup rather than drawing.

Tolerable, and the strongest concrete argument for the **`blend2d` CPU raster
backend** the parent crate already has a feature placeholder for
(`Cargo.toml`, and the note in the root `CLAUDE.md`). This crate would be its
first real consumer: swapping the backend would cut the per-file cost to the
drawing alone and remove the dependency on a working adapter, which also
matters on a headless or driverless box where the fallback is llvmpipe if it is
there at all.

Until then, a machine with no adapter simply gets no thumbnails — the binary
exits non-zero and the file manager shows its generic icon, which is the right
degradation.

## Checking it

```sh
cargo test --release           # aspect, opacity, PNG, failure paths, arg parsing
cargo build --release

# exactly what the .thumbnailer Exec line expands to
./target/release/hephaestus-thumbnailer -s 512 -i /path/plot.hep -o /tmp/t.png
```

Then look at `/tmp/t.png` — this is the one shell integration in the repo whose
output can simply be inspected.

What **cannot** be checked here is the last step: whether a file manager picks
it up. That needs a Linux desktop, `../hephaestus-viewer/package-linux.sh`, and
afterwards:

```sh
sudo update-mime-database /usr/share/mime
nautilus -q            # or: pkill tumblerd
rm -rf ~/.cache/thumbnails
```

## Not built yet

- **`Thumb::URI` and `Thumb::MTime` PNG text chunks.** The freedesktop cache
  format wants them for validation, but the thumbnail *factories* (both
  gnome-desktop and tumbler) write them after invoking the thumbnailer, so a
  plain PNG should be right. Unverified, and the failure mode if it is wrong is
  benign — thumbnails regenerate more often than needed. This crate's PNG
  writer takes only a dpi, so adding them would mean reaching past it.
- **A KDE/Dolphin thumbnailer.** A C++ KIO plugin; see above.
- **The `blend2d` backend**, as above.
- **Anything about packaging in CI.** The `.thumbnailer` and the binary are
  mapped into the deb and rpm `files` lists in
  `../hephaestus-viewer/tauri.conf.json`, but nobody has run
  `package-linux.sh`, so the exact install layout is written from the config
  schema and the specs rather than observed.

## Cross-references

- `../hephaestus-quicklook/CLAUDE.md` — the macOS equivalents, and how much
  more they cost.
- `../hephaestus-viewer/CLAUDE.md` — the app, the MIME declaration, and
  `package-linux.sh`.
- `../../src/backend/hybrid/CLAUDE.md` — the rasterizer this uses, and why it
  rather than the compute-shader one.
