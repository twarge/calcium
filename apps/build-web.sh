#!/usr/bin/env bash
# Builds the WebAssembly engine and assembles the static demo in web/.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --profile wasm-release --target wasm32-unknown-unknown -p calcium-ffi
cp target/wasm32-unknown-unknown/wasm-release/calcium_ffi.wasm web/
ls -la web/
echo "Serve web/ from any static host. The wasm is ~$(du -h web/calcium_ffi.wasm | cut -f1) (~130 KB gzipped)."
