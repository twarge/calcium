#!/usr/bin/env bash
# Builds the WebAssembly demo and installs it into the twargeweb site:
#
#   ../twargeweb/source/calcium/          the page, script, and wasm module
#   ../twargeweb/source/fonts/firacode/   Fira Code as woff2, licence alongside
#
# The page itself (index.md) lives in the site repo and is not overwritten if
# it already exists — the site owns its prose; this script owns the artifacts.
set -euo pipefail
cd "$(dirname "$0")/.."

SITE="../twargeweb"
DEST="$SITE/source/calcium"
FONTS="$SITE/source/fonts/firacode"
[ -d "$SITE/source" ] || { echo "twargeweb not found at $SITE" >&2; exit 1; }

echo "==> wasm engine"
cargo build --profile wasm-release --target wasm32-unknown-unknown -p calcium-wasm
mkdir -p "$DEST" "$FONTS"
cp target/wasm32-unknown-unknown/wasm-release/calcium_wasm.wasm "$DEST/calcium_ffi.wasm"
cp web/calcium.js "$DEST/"

echo "==> fonts (woff2)"
for weight in Regular Bold; do
  if [ ! -f "$FONTS/FiraCode-$weight.woff2" ]; then
    python3 - "$weight" "$FONTS" <<'PY'
import sys
from fontTools.ttLib import TTFont
weight, dest = sys.argv[1], sys.argv[2]
font = TTFont(f"apps/Calcium/Resources/Fonts/FiraCode-{weight}.ttf")
font.flavor = "woff2"
font.save(f"{dest}/FiraCode-{weight}.woff2")
PY
  fi
done
cp apps/Calcium/Resources/Fonts/LICENSE-FiraCode.txt "$FONTS/"

echo "==> page"
if [ ! -f "$DEST/index.md" ]; then
  echo "index.md not present — copy web/site-index.md as a starting point" >&2
fi

ls -la "$DEST" "$FONTS"
echo "Done. Build the site with: (cd $SITE && cobalt build)"
