#!/usr/bin/env bash
# Builds the web version into web/dist: a static site (HTML/JS/WASM) where the
# compression runs in the browser. Serve it with any static file server, e.g.
#   python3 -m http.server -d web/dist 8000
# (not from file://: module workers and WebAssembly need http).
#
# Needs the wasm32-unknown-unknown target and a wasm-bindgen CLI matching the
# wasm-bindgen version in Cargo.lock:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version in Cargo.lock> --locked
# wasm-opt (binaryen), when installed, shrinks the module further.
set -euo pipefail

cd "$(dirname "$0")/.."
OUT=web/dist

cargo build -p pdfshrink-wasm --target wasm32-unknown-unknown --release

rm -rf "$OUT"
mkdir -p "$OUT/pkg"
wasm-bindgen --target web --no-typescript --out-dir "$OUT/pkg" \
  target/wasm32-unknown-unknown/release/pdfshrink_wasm.wasm

if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int \
    "$OUT/pkg/pdfshrink_wasm_bg.wasm" -o "$OUT/pkg/pdfshrink_wasm_bg.wasm"
fi

# The page and its scripts, plus the desktop app's stylesheet and
# translations, shared as-is.
cp web/index.html web/app.js web/worker.js web/web.css "$OUT/"
cp app/ui/style.css app/ui/i18n.js "$OUT/"

echo "Built $OUT ($(du -sh "$OUT" | cut -f1))"
