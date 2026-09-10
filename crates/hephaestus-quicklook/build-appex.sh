#!/usr/bin/env bash
#
# Build the two Quick Look extensions: the space-bar preview and the Finder
# thumbnail.
#
# There is no Xcode project here on purpose. An `.appex` is a bundle directory
# with an executable and an Info.plist, and `swiftc` from the Command Line
# Tools can produce the executable — so both are assembled by hand and there
# is no `.xcodeproj` to keep in sync with the Rust build. The one non-obvious
# linker flag is the entry point: an app extension has no `main`, and
# `_NSExtensionMain` in Foundation is what runs instead.
#
# **Two bundles, not one.** An app extension declares exactly one
# `NSExtensionPointIdentifier`, and previews and thumbnails are different
# points. All the implementation is shared (`appex/PlotDocument.swift`); what
# is duplicated is two Info.plists.
#
#   ./build-appex.sh                         # ad-hoc signed, this machine only
#   ./build-appex.sh "Apple Development: …"   # signed with a real identity
#   HEPHAESTUS_QL_DIAG=1 ./build-appex.sh     # …with the diagnostic channel
#
# Output: `target/HephaestusQuickLook.appex` and
# `target/HephaestusThumbnail.appex`. `package-macos.sh` in
# `../hephaestus-viewer` is what puts them inside the app.

set -euo pipefail
cd "$(dirname "$0")"

IDENTITY="${1:-${HEPHAESTUS_SIGN_IDENTITY:--}}"
# macOS 12 is where the data-based reply arrived; it is the floor for the whole
# approach, not an arbitrary choice.
DEPLOYMENT_TARGET="12.0"
SDK="$(xcrun --sdk macosx --show-sdk-path)"

# A sandboxed extension has almost no way to report anything, so it can be
# built with a diagnostic channel that writes into its own container. Off by
# default: a preview should not touch the disk.
DIAG_FLAG=""
if [ "${HEPHAESTUS_QL_DIAG:-0}" = "1" ]; then
  DIAG_FLAG="-D HEP_QL_DIAG"
  echo "==> diagnostics ON: ~/Library/Containers/<bundle-id>/Data/tmp/hep-ql-diag.txt"
fi

echo "==> cargo build --release"
cargo build --release

# Whatever the static library needs, straight from rustc rather than guessed.
# CoreText is in there because `parley` is a non-optional dependency of the
# crate, so a document's fonts resolve against the system's.
NATIVE_LIBS=$(
  cargo rustc --release --lib -- --print native-static-libs 2>&1 |
    sed -n 's/^note: native-static-libs: //p' | tail -1
)
echo "==> native libs: ${NATIVE_LIBS}"

# build <module> <plist> <framework> <source…>
build_appex() {
  local module="$1"; shift
  local plist="$1"; shift
  local framework="$1"; shift
  local out="target/${module}.appex"

  echo "==> assembling ${out}"
  rm -rf "$out"
  mkdir -p "$out/Contents/MacOS"
  cp "$plist" "$out/Contents/Info.plist"

  # shellcheck disable=SC2086
  xcrun swiftc \
    -sdk "$SDK" \
    -target "$(uname -m)-apple-macos${DEPLOYMENT_TARGET}" \
    -module-name "$module" \
    -import-objc-header appex/bridge.h \
    -application-extension \
    -O -wmo \
    $DIAG_FLAG \
    -o "$out/Contents/MacOS/$module" \
    "$@" \
    -L target/release -lhephaestus_quicklook \
    -framework "$framework" \
    $NATIVE_LIBS \
    -Xlinker -e -Xlinker _NSExtensionMain

  echo "==> codesign ${module} (identity: ${IDENTITY})"
  codesign --force --timestamp=none \
    --sign "$IDENTITY" \
    --entitlements appex/entitlements.plist \
    --options runtime \
    "$out"
  codesign --verify --strict --verbose=2 "$out"
}

build_appex HephaestusQuickLook appex/Preview-Info.plist QuickLookUI \
  appex/PlotDocument.swift appex/PreviewProvider.swift

build_appex HephaestusThumbnail appex/Thumbnail-Info.plist QuickLookThumbnailing \
  appex/PlotDocument.swift appex/ThumbnailProvider.swift

echo "==> built both extensions"
