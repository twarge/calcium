#!/usr/bin/env bash
# Generates the Xcode project and builds the app.
#
#   ./apps/build.sh            debug build
#   ./apps/build.sh release    release build
set -euo pipefail

cd "$(dirname "$0")"
CONFIG="${1:-debug}"
case "$CONFIG" in
  debug)   XC_CONFIG=Debug ;;
  release) XC_CONFIG=Release ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

# The engine is always built optimised: it is exercised by `cargo test` rather
# than by stepping through it, and unoptimised it is six times slower to
# evaluate a document.
echo "==> Rust engine (release)"
(cd .. && cargo build --release -p calcium-ffi)

echo "==> Xcode project"
xcodegen generate --quiet

echo "==> App ($XC_CONFIG)"
set +e
xcodebuild -project Calcium.xcodeproj -scheme Calcium -configuration "$XC_CONFIG" \
  -derivedDataPath build build 2>&1 | grep -E "error:|warning:|BUILD (SUCCEEDED|FAILED)" | sort -u
STATUS=${PIPESTATUS[0]}
set -e

APP="build/Build/Products/$XC_CONFIG/Calcium.app"
if [ "$STATUS" -ne 0 ]; then
  echo "==> Build failed" >&2
  exit "$STATUS"
fi
echo "==> Built $(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")"
