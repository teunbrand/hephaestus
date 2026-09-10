# hephaestus-quicklook

The macOS Quick Look extensions for `.hep` plot documents: press space on one
in Finder to see the plot, and see it as the file's icon too.

Three small pieces — a Rust static library that turns document bytes into PDF
bytes, and two Swift app extensions that consume it: one hands the PDF to
Quick Look's preview panel, the other rasterizes it through Core Graphics for
Finder's thumbnails. Two bundles because an app extension declares exactly one
extension point; all the implementation is shared.

None of it needs a GPU or an Xcode project. The parent crate's renderer-free
`document-read,pdf` build does the work, and `build-appex.sh` assembles both
bundles with `swiftc`.

```sh
cargo test --release        # document -> PDF
./build-appex.sh            # build and ad-hoc sign both .appex bundles

# the one end-to-end check, and it writes a PNG you can look at
xcrun swiftc -O verify-thumbnail.swift -o target/verify-thumbnail
./target/verify-thumbnail some.hep /tmp/t.png 512 2
```

`../hephaestus-viewer/package-macos.sh` builds the app with both inside it.

**Read `CLAUDE.md` before changing anything here.** Three of the four things
that make a preview extension load fail silently; a thumbnail reply's context
has a trap in its coordinate space; and `qlmanage` verifies neither on macOS 26
— it crashes on Apple's own extensions too. All of it is written up there, with
the control experiments.
