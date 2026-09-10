# hephaestus-thumbnailer

Renders a `.hep` plot document to a thumbnail image.

The binary is a **freedesktop thumbnailer**, which is how Linux file managers
get thumbnails: a data file in `/usr/share/thumbnailers/` names a command line
and the file manager runs it. No plugin ABI, no registration, no signing.

```sh
cargo test --release
cargo build --release
./target/release/hephaestus-thumbnailer -s 512 -i plot.hep -o /tmp/t.png
```

The library is the same rendering without a process boundary, for a future
Windows `IThumbnailProvider` — a COM DLL, which cannot shell out.

Read by GNOME Files and Thunar; **not** by KDE's Dolphin, which needs its own
C++ plugin. `../hephaestus-viewer/package-linux.sh` builds the packages with
both halves installed. See `CLAUDE.md`.
