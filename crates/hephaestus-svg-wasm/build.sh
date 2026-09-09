#!/usr/bin/env bash
# Assemble the publishable package in dist/.
#
# One layout for development and publication: the demo in www/ imports
# ../dist/hephaestus-svg.js, which is the same entry point npm and a CDN
# serve. Keeping the wrapper in a directory of its own would mean it imported
# the glue by a different path in each case, which is exactly the discrepancy
# that only shows up after publishing.
#
#   ./build.sh            release build
#   ./build.sh --dev      faster build, panic messages, no wasm-opt
set -euo pipefail
cd "$(dirname "$0")"

# A plain string rather than an array: macOS ships bash 3.2, where expanding
# an empty array under `set -u` is an error.
profile=release
extra=""
if [[ "${1:-}" == "--dev" ]]; then
  profile=dev
  extra="--features debug-panics"
fi

# `--no-pack` because wasm-pack's own package.json lists only its three
# outputs and points `main` at the glue rather than at our wrapper.
# shellcheck disable=SC2086
wasm-pack build --target web --"$profile" --out-dir dist --no-pack $extra

cp js/hephaestus-svg.js dist/hephaestus-svg.js
cp js/hephaestus-svg.d.ts dist/hephaestus-svg.d.ts

# The same faces the canvas client bundles, taken from there rather than
# copied into this crate: they are produced by one `fonts/generate.sh`, and a
# second checked-in copy would be half a megabyte of binary that could drift.
# The licence travels with them, as OFL requires.
mkdir -p dist/fonts
cp ../hephaestus-wasm/fonts/roboto-*.ttf ../hephaestus-wasm/fonts/OFL-Roboto.txt dist/fonts/

# The npm version tracks the crate version, so there is one number to bump.
python3 - <<'PY'
import json, re, pathlib
cargo = pathlib.Path("Cargo.toml").read_text()
version = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.M).group(1)
pkg = json.loads(pathlib.Path("package.template.json").read_text())
pkg["version"] = version
pathlib.Path("dist/package.json").write_text(json.dumps(pkg, indent=2) + "\n")
print(f"dist/package.json at version {version}")
PY

echo "dist/ contents:"
ls -1 dist/
