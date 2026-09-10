#!/usr/bin/env bash
#
# Build the Linux packages with the thumbnailer in them.
#
# Two things have to be installed for a `.hep` to get a thumbnail in a file
# manager, and `tauri.conf.json` maps both into the deb and rpm `files` lists:
#
#   /usr/share/thumbnailers/hephaestus-plot.thumbnailer   the data file
#   /usr/bin/hephaestus-thumbnailer                       the program it names
#
# The bundler copies the binary from the sibling crate's `target/`, so that
# crate has to be built *first* — which is the only reason this script exists
# rather than a bare `cargo tauri build`.
#
# A third piece is already in the map: the shared-mime-info XML that declares
# `application/x-hephaestus-plot`, without which the desktop has a handler and
# a thumbnailer for a type it does not recognize.
#
#   ./package-linux.sh              # deb and rpm
#   ./package-linux.sh appimage     # …or whichever bundle targets
#
# **Untested.** Nobody has run this on Linux; it is written from the config
# schema and the freedesktop specs. See CLAUDE.md.

set -euo pipefail
cd "$(dirname "$0")"

BUNDLES="${1:-deb,rpm}"

echo "==> building the thumbnailer (the bundler copies it from target/)"
( cd ../hephaestus-thumbnailer && cargo build --release )

echo "==> cargo tauri build --bundles ${BUNDLES}"
cargo tauri build --bundles "$BUNDLES"

cat <<'NOTE'

==> after installing the package, the caches have to be told:

  sudo update-mime-database /usr/share/mime
  sudo update-desktop-database

A file manager may also need restarting before it notices a new thumbnailer
(`nautilus -q`, or `pkill tumblerd` for Thunar). Existing files keep their old
generic icon until the thumbnail cache is cleared:

  rm -rf ~/.cache/thumbnails
NOTE
