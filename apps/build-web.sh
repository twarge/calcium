#!/usr/bin/env bash
# Builds the WebAssembly engine and assembles the static demo in web/.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --profile wasm-release --target wasm32-unknown-unknown -p calcium-wasm
cp target/wasm32-unknown-unknown/wasm-release/calcium_wasm.wasm web/calcium_ffi.wasm
ls -la web/
echo "Serve web/ from any static host. The wasm is $(du -h web/calcium_ffi.wasm | cut -f1) ($(gzip -c web/calcium_ffi.wasm | wc -c | awk '{printf "%d KB", $1 / 1024}') gzipped)."
