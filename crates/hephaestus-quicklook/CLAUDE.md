# crates/hephaestus-quicklook/CLAUDE.md

The macOS Quick Look extensions for `.hep` documents: press space in Finder
and see the plot, and see it as the file's icon too. Three artifacts, all
small:

- **A Rust static library** (`src/lib.rs`) — two `extern "C"` functions taking
  document bytes to PDF bytes.
- **A preview extension** (`appex/PreviewProvider.swift`) — hands that PDF to
  Quick Look for the space-bar panel.
- **A thumbnail extension** (`appex/ThumbnailProvider.swift`) — rasterizes the
  same PDF through Core Graphics for Finder's icons.

**Two bundles, not one, and not by choice:** an app extension declares exactly
one `NSExtensionPointIdentifier`, and previews and thumbnails are different
points. Everything below the two principal classes is shared
(`appex/PlotDocument.swift`), so what is duplicated is two `Info.plist`s rather
than two implementations. `build-appex.sh` builds both with `swiftc` and no
Xcode project; `../hephaestus-viewer/package-macos.sh` puts both inside the app
bundle, because Tauri's bundler has no concept of an app extension.

## Why this is small

Two facts do all the work.

**Since macOS 12 Quick Look accepts a data reply rather than a view.**
`QLPreviewReply(dataOfContentType:contentSize:dataCreationBlock:)` means an
extension that can produce a PDF needs no UI code, no view controller, no
layout and no drawing — so there is no AppKit here beyond the class
declaration.

**The parent crate already has a renderer-free PDF path.**
`--no-default-features --features document-read,pdf` pulls no wgpu, no
rasterizer and no GPU — which an extension has no business opening — and the
PDF backend *embeds every glyph it draws*, so a preview looks right on a
machine with none of the plot's fonts. Quick Look then scales the vector
content to whatever size the panel is. PDF is the right answer here, not
merely a convenient one.

The engine is therefore the same code the viewer's vector export uses, at the
same `dpi = 72` so the page size *is* the MediaBox.

**The thumbnail cost no new Rust at all.** It needs pixels, but the extension
has no GPU and no rasterizer — so Core Graphics rasterizes the PDF instead
(`CGPDFDocument` + `drawPDFPage`), which is sharp at any icon size and reuses
the preview's engine verbatim. That is the payoff for having factored the
engine behind two C functions.

## Nothing may unwind across the boundary

A panic crossing an `extern "C"` frame is undefined behaviour, and this runs
inside a system extension host, so both entry points wrap their work in
`catch_unwind`. That is also why `[profile.release]` keeps `panic = "unwind"`:
`panic = "abort"` would turn a malformed document into a dead extension host.

A document is attacker-controlled — somebody was sent a file — and the reader
is hand-rolled, so this is a real path rather than a formality.
`tests/pdf_from_document.rs` feeds it empty input, rubbish, and a real
document cut in half.

Failure is a null `data` pointer and nothing else. There is nowhere in a Quick
Look panel to report a message, and a null makes the Swift side throw, which
makes Finder fall back to its generic icon — the right degradation, and better
than a blank panel that reads as a hang.

## The four things that make it load

None of these are in the Swift, and three of them fail *silently*. This is the
section to read before debugging anything.

### 1. `QLIsDataBasedPreview` — required, undocumented, preview only

(The thumbnail bundle needs no counterpart: thumbnail providers have only one
flavour. Everything else in this section applies to both.)

Quick Look supports two kinds of preview extension behind one extension point.
View-based ones have an `NSViewController` as their principal class;
data-based ones subclass `QLPreviewProvider` and hand back bytes. **This
`NSExtensionAttributes` flag is how Quick Look tells them apart**, and it does
not appear in Apple's documentation of the API.

Omit it and the default is view-based: Quick Look asks a `QLPreviewProvider`
for a view controller, gets nothing, and reports that the file "did not
produce any preview" — with `providePreview` never called, no error, and
nothing readable in the log.

Found by reading the `Info.plist` of a shipping extension. That is the general
technique here and it is worth remembering:

```sh
# every preview extension on the system, and which kind it is
find /System/Applications /Applications /System/Library -name Info.plist -path '*.appex/*' 2>/dev/null |
  while read -r p; do
    [ "$(plutil -extract NSExtension.NSExtensionPointIdentifier raw -o - "$p" 2>/dev/null)" \
      = com.apple.quicklook.preview ] &&
      echo "$(plutil -extract NSExtension.NSExtensionAttributes.QLIsDataBasedPreview raw -o - "$p" 2>/dev/null)  $p"
  done
```

`/Applications/Xcode.app/Contents/PlugIns/ProvisoningProfileQuicklookExtension.appex`
is the reference to copy: a minimal data-based extension whose `Info.plist` is
key-for-key what this one is.

### 2. `CFBundleSupportedPlatforms`

Xcode writes it and assembling a bundle by hand is how you find out it matters.
Without it, ExtensionFoundation throws `NSInvalidArgumentException` — "key
cannot be nil" — from `-[EXConcreteExtension
makeExtensionContextAndXPCConnectionForRequest:error:]`, which surfaces as the
*host process crashing* rather than as anything about this bundle.

### 3. The principal class name is an Objective-C name

`NSExtensionPrincipalClass` is resolved with `NSClassFromString`. A Swift class
is registered under a mangled name — `_TtC19HephaestusQuickLook15PreviewProvider`
— so the class carries `@objc(PreviewProvider)` and the plist says
`PreviewProvider`. `Module.Class` also works (Apple's own extensions use it),
but the explicit `@objc` name removes the question. A miss fails silently.

Check with `nm … | grep '_OBJC_CLASS_\$_'`.

### 4. Signing is inside-out

A bundle's signature covers its nested code, so the appex has to be sealed
*before* the app is signed over it. Signing the app first leaves its seal
describing something that is no longer there, and the failure is Finder quietly
showing a generic icon.

`--deep` is not the answer and is deprecated: it would re-sign the appex with
the *app's* entitlements, dropping the App Sandbox the extension must have.
`package-macos.sh` signs the appex, embeds it, then signs the app.

**The sandbox is mandatory** for a Quick Look extension — an unsandboxed one is
refused rather than warned about. `appex/HephaestusQuickLook.entitlements` is
the whole of it: the sandbox, plus user-selected read-only. Nothing else is
needed, since the renderer touches no network, writes nothing and opens no
device.

## Preview and thumbnail differ in one interesting way

**The thumbnail is told what size to render; the preview is not.**
`QLFileThumbnailRequest` carries `maximumSize`, `minimumSize` and `scale`,
where `QLFilePreviewRequest` carries nothing but a `fileURL`. So a thumbnail is
rasterized to fit exactly, while a preview has to pick a size and be scaled —
which is the whole content of the next section.

The reply carries the plot's own aspect rather than filling the box Quick Look
offered: a smaller context is accepted and centred, so reporting the true
aspect beats distorting, and beats padding — padding would put the thumbnail's
visible edges in the wrong place in Finder's gallery view.

### The points-versus-pixels trap in a thumbnail reply

`QLThumbnailReply(contextSize:drawingBlock:)` produces a bitmap of
`contextSize × request.scale` pixels — **but the context arrives with an
identity CTM.** So drawing in the units `contextSize` was expressed in covers
`1/scale²` of the image, anchored bottom-left: at 2×, a quarter of the
thumbnail with the plot in the bottom-left corner and three-quarters blank.
Found by looking at the output, which is the only way it *would* be found —
nothing errors.

`PlotDocument.drawableArea` sidesteps the question rather than picking a
convention: it asks the context how large it is
(`boundingBoxOfClipPath`, with a fallback for an unbounded clip) and draws into
that. Correct under either convention, and immune to the answer changing.

## The preview scales; it does not reflow

A `.hep` reflows everywhere else in this repo, so this is the one place it
does not, and it is worth knowing why before trying to fix it.

**The data-based API cannot reflow, and the reason is structural.** Checked
against the headers rather than inferred:

- `QLFilePreviewRequest` has exactly one property, `fileURL`. Quick Look never
  tells the extension how large the panel is — not on resize, and **not even
  for the first render**.
- `QLPreviewingController` has three optional methods, all one-shot
  prepare/provide calls. There is no resize hook.
- Every `QLPreviewReply` initializer takes its size at construction. The reply
  is one immutable document.

So the API's shape is "produce one document at a size you choose", and the size
chosen here is the document's own hint — the size the plot was composed for,
which is the most faithful answer available.

What that costs is *reflow*: the layout is solved once, so text keeps its
proportion to the plot instead of re-wrapping, and tick counts do not adapt to
the panel. What it does **not** cost is sharpness — the reply is vector PDF, so
Quick Look scales it cleanly at any size. That is also what previewing a PDF or
an image does, and no Quick Look panel reflows, so scaling is arguably the
correct behaviour rather than a limitation to route around.

**Reflow would mean a view-based extension**: `QLIsDataBasedPreview = false`
and an `NSViewController` as the principal class, which does get real layout
callbacks, so the PDF could be regenerated on resize. That is a materially
bigger thing — a view controller, a PDF view, debounced re-rendering — inside a
sandboxed extension with a few hundred milliseconds of budget, and re-solving a
dense plot on every step of a resize drag is not obviously a good idea. Not
built, deliberately.

## `qlmanage` cannot verify this, and that is not our bug

**On macOS 26, both `qlmanage -p` and `qlmanage -x -p` die on *any* data-based
preview extension**, with:

```
*** Terminating app due to uncaught exception 'NSInvalidArgumentException',
    reason: '*** -[__NSDictionaryM setObject:forKey:]: key cannot be nil'
    … -[EXConcreteExtension makeExtensionContextAndXPCConnectionForRequest:error:]
```

Established by control experiment, which is the only reason it is stated as
fact: previewing a `.ips` crash report — handled by Apple's own
`OSAnalytics.framework/PlugIns/IPSExtension.appex`, which declares
`QLIsDataBasedPreview = true` — fails **identically**, while a PNG previews
fine through the same command. `QLThumbnailGenerator` (`verify-preview.swift`)
fails on both too, because a thumbnail is not derived from a data-based
preview.

So a `qlmanage` failure says nothing about the extension under test. **Do not
spend time "fixing" it.** Run the control before believing any negative result:

```sh
IPS=$(find ~/Library/Logs/DiagnosticReports -name '*.ips' | head -1)
qlmanage -x -p -o /tmp/out "$IPS"     # crashes on stock macOS 26
```

**For the preview, the only verification is Finder: select a `.hep` and press
space.**

**The thumbnail, however, *is* checkable** — `QLThumbnailGenerator` goes
through a different path and works, which is what `verify-thumbnail.swift` is
for. It writes a PNG, so the result can actually be looked at:

```sh
xcrun swiftc -O verify-thumbnail.swift -o target/verify-thumbnail
./target/verify-thumbnail some.hep /tmp/t.png 512 2      # file, out, points, scale
```

Rendering across Finder's range (512 down to 16 points) is also how the
small-icon cases get checked; at 128 pt the plot is still recognizable and the
text is not, which is the right degradation.

Note that **`qlmanage -t` is no use either** — for this type it prints nothing,
writes nothing and exits 0. Use the tool above.

## What *can* be checked from a script

```sh
cd crates/hephaestus-quicklook
cargo test --release            # document -> PDF, embedded fonts, malformed input
./build-appex.sh                # ad-hoc signed
HEPHAESTUS_QL_DIAG=1 ./build-appex.sh   # …with the diagnostic channel
```

and against a packaged app:

```sh
APP=".../Hephaestus Viewer.app"
# the type is declared and the system agrees
mdls -name kMDItemContentType /path/to/plot.hep      # dev.posit.hephaestus.plot
# both extensions registered, parented to the app, on the right points
pluginkit -m | grep hephaestus
pluginkit -m -i dev.posit.hephaestus.viewer.quicklook -vvv
pluginkit -m -i dev.posit.hephaestus.viewer.thumbnail -vvv
# the signature is valid with the appex sealed inside
codesign --verify --strict --deep --verbose=2 "$APP"
# the sandbox survived the outer signing, for each
codesign -d --entitlements - "$APP/Contents/PlugIns/HephaestusQuickLook.appex"
codesign -d --entitlements - "$APP/Contents/PlugIns/HephaestusThumbnail.appex"
# the class and selector Quick Look looks for exist
nm "$APP/Contents/PlugIns/.../HephaestusQuickLook" | grep '_OBJC_CLASS_\$_PreviewProvider'
otool -v -s __TEXT __objc_methname "…/HephaestusQuickLook" | grep providePreview
```

Registration needs the app somewhere Launch Services scans. From a build
directory it does not happen on its own:

```sh
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP"
pluginkit -a "$APP/Contents/PlugIns/HephaestusQuickLook.appex"
pluginkit -a "$APP/Contents/PlugIns/HephaestusThumbnail.appex"
qlmanage -r; qlmanage -r cache    # drop Quick Look's generator list and thumbnail cache
```

### The diagnostic channel

A sandboxed appex has almost no way to report anything — the unified log needs
privileges to read, `print` goes nowhere, and a failure appears only as a
generic icon in Finder. So `HEPHAESTUS_QL_DIAG=1 ./build-appex.sh` compiles in
a `diag()` that appends to a file inside the extension's own container:

```
~/Library/Containers/dev.posit.hephaestus.viewer.quicklook/Data/tmp/hep-ql-diag.txt
```

Off by default — a preview should not touch the disk — and it costs ~4 kB when
on. **It has not been exercised end to end**, because no scriptable route
reaches the extension at all; it is a tool for whoever next debugs this from
Finder.

## Signing and distribution, as it actually stands

- **Local use works with an Apple Development identity.** That is what the
  extension has been signed with here (Team ID present, sandbox attached,
  `codesign --verify --strict --deep` clean).
- **Distribution needs a Developer ID Application certificate** — a different
  certificate from the same account — and then notarization
  (`xcrun notarytool submit` on a zip or dmg, then `xcrun stapler staple`).
  Neither is done, and neither is scripted.
- Ad-hoc (`-`) signing produces a bundle that verifies and registers, so it is
  fine for a build-and-look loop.
- `package-macos.sh` takes the identity as its first argument, so switching is
  one word.

## Not built yet

- **A universal binary.** `build-appex.sh` targets the build machine's
  architecture. Distribution wants `arm64` + `x86_64` and a `lipo` step; the
  Rust side needs the matching `--target`s.
- **Notarization**, as above.
- **Anything about the appex in CI.** It needs a macOS runner and a signing
  identity in the keychain, so `check.yml` does not build it. The Rust half is
  covered by `cargo test`, which is the part that can rot silently.

## Cross-references

- `../hephaestus-viewer/CLAUDE.md` — the app this lives inside, the packaging
  script, and where the UTI is declared: `dev.posit.hephaestus.plot`, conforming
  to `public.image` so Finder groups a `.hep` with pictures. Both extensions'
  `QLSupportedContentTypes` must name that same string.
- `../../src/backend/pdf/CLAUDE.md` — the backend that makes this possible:
  why the embedded font is synthesized rather than sliced.
- `../../src/document/CLAUDE.md` — what a document carries, and why an
  unreadable one is a version mismatch rather than corruption.
