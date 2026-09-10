#!/usr/bin/env bash
#
# Build the macOS app with the Quick Look extension inside it, signed.
#
# Tauri's bundler does not know about app extensions — there is no config for
# one — so this drives it and then does the three things it cannot: build the
# appex, put it in `Contents/PlugIns/`, and re-sign.
#
# **Signing is inside-out and that is not a style preference.** A bundle's
# signature covers its nested code, so the appex has to be sealed before the
# app is signed over it. Signing the app first and the appex afterwards leaves
# the app's seal describing something that is no longer there, and the failure
# is Finder quietly showing a generic icon rather than an error.
#
#   ./package-macos.sh                        # ad-hoc, this machine only
#   ./package-macos.sh "Apple Development: …"   # a real identity
#   ./package-macos.sh "Developer ID Application: …"   # for distribution
#
# Ad-hoc is enough to see a preview locally. Distribution needs a Developer ID
# identity and then notarization, which is a separate step this does not do —
# see CLAUDE.md.

set -euo pipefail
cd "$(dirname "$0")"

IDENTITY="${1:-${HEPHAESTUS_SIGN_IDENTITY:--}}"
APP="target/release/bundle/macos/Hephaestus Viewer.app"
APPEX_DIR="../hephaestus-quicklook/target"
# Two of them: an app extension declares exactly one extension point, and the
# space-bar preview and the Finder thumbnail are different points.
APPEXES="HephaestusQuickLook.appex HephaestusThumbnail.appex"

echo "==> cargo tauri build"
cargo tauri build --bundles app

echo "==> building the Quick Look extensions"
( cd ../hephaestus-quicklook && ./build-appex.sh "$IDENTITY" )

echo "==> embedding the extensions"
mkdir -p "$APP/Contents/PlugIns"
for appex in $APPEXES; do
  rm -rf "$APP/Contents/PlugIns/$appex"
  cp -R "$APPEX_DIR/$appex" "$APP/Contents/PlugIns/"
done

echo "==> signing the app over it (identity: ${IDENTITY})"
# No `--deep`: it is deprecated and it would re-sign the already-sealed appex
# with the *app's* entitlements, dropping the sandbox the extension must have.
codesign --force --timestamp=none \
  --sign "$IDENTITY" \
  --entitlements macos/entitlements.plist \
  --options runtime \
  "$APP"

echo "==> verifying"
codesign --verify --strict --deep --verbose=2 "$APP"
echo
for appex in $APPEXES; do
  echo "$appex:"
  codesign -dv "$APP/Contents/PlugIns/$appex" 2>&1 | grep -E "Identifier|TeamIdentifier" | sed 's/^/  /' || true
done
echo
echo "==> built $APP"
echo
echo "To let Launch Services see the extensions:"
echo "  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f \"$APP\""
echo "  qlmanage -p /path/to/plot.hep       # a preview window"
echo "  pluginkit -m | grep hephaestus"
echo "  qlmanage -t -s 512 -o /tmp /path/to/plot.hep    # the thumbnail, as a file"
