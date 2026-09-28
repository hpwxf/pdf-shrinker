#!/usr/bin/env bash
# Builds PdfShrinker.app and its .dmg installer for Apple Silicon only
# (aarch64-apple-darwin — no x86_64/universal build).
#
# Usage: scripts/build-dmg.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="aarch64-apple-darwin"

cd "$ROOT"

echo "==> Building the CLI (release, $TARGET)"
cargo build --release -p pdfshrink-cli --target "$TARGET"

echo "==> Staging it as the app's sidecar binary"
mkdir -p app/src-tauri/binaries
cp "target/$TARGET/release/pdfshrink" "app/src-tauri/binaries/pdfshrink-$TARGET"

echo "==> Building the app + .dmg (ad-hoc signed)"
(cd app && cargo tauri build --target "$TARGET" --bundles dmg)

DMG_DIR="target/$TARGET/release/bundle/dmg"
echo
echo "==> Done:"
ls -1 "$DMG_DIR"/*.dmg 2>/dev/null || echo "(no .dmg found in $DMG_DIR — check the build output above)"

# --- Notarization (not done yet) ------------------------------------------
# The app and dmg above are only ad-hoc signed (tauri.conf.json's
# `bundle.macOS.signingIdentity: "-"`), so Gatekeeper will warn on first
# launch on another Mac. Once a Developer ID is available:
#   1. Set `signingIdentity` to the "Developer ID Application: …" identity
#      (and set APPLE_SIGNING_IDENTITY / APPLE_CERTIFICATE* env vars, or let
#      `cargo tauri build` read them) instead of "-".
#   2. Notarize the .dmg:
#      xcrun notarytool submit "$DMG_DIR"/*.dmg --keychain-profile "<profile>" --wait
#      xcrun stapler staple "$DMG_DIR"/*.dmg
