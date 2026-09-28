# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A macOS PDF compressor (Rust), à la iLovePDF/UPDF, ships three ways from one Cargo workspace:
CLI (`pdfshrink`), a Tauri GUI (`PdfShrinker.app`), and a Finder Quick Action — all going through the
same `pdfshrink-core` compression engine so behavior never drifts between front ends. Apple Silicon
only (`aarch64-apple-darwin`); no x86_64/universal build.

## Commands

```bash
# Core engine: build, test, lint
cargo build -p pdfshrink-core
cargo test -p pdfshrink-core                 # fixtures are synthesized in-test, nothing binary is committed
cargo test -p pdfshrink-core smask_is_resized_alongside_its_parent_image   # single test
cargo clippy -p pdfshrink-core --all-targets

# CLI: build, run
cargo build -p pdfshrink-cli
cargo run -p pdfshrink-cli -- -l medium file.pdf
cargo run -p pdfshrink-cli -- config get level
cargo run -p pdfshrink-cli -- install --quick-action --cli-link   # writes to ~/Library/Services and /usr/local/bin — real side effects, don't run from CI/agents without asking

# Generate a throwaway test PDF (oversized JPEG on one page) instead of needing a real file
cargo run -p pdfshrink-core --example make_fixture -- /path/to/out.pdf

# App: dev loop, build .app (needs the CLI sidecar staged first — see below)
cd app && cargo tauri dev
cd app && cargo tauri build --target aarch64-apple-darwin --bundles dmg

# Full .app + .dmg (stages the sidecar, builds, ad-hoc signs)
./scripts/build-dmg.sh
```

Running the app (`cargo tauri build`/`dev`) requires `app/src-tauri/binaries/pdfshrink-aarch64-apple-darwin`
to exist (Tauri's `externalBin` resource check fails otherwise): build the CLI for that target and copy
it there, or just run `scripts/build-dmg.sh` which does it. That directory is gitignored.

Validate a compressed PDF structurally with `qpdf --check out.pdf` (installed via Homebrew on this
machine) and visually with `pdftoppm -png out.pdf preview`.

## Architecture

### `crates/pdfshrink-core` — the only place compression logic lives

`compress_file(input, &CompressOptions) -> Result<Outcome>` (`compress.rs`) is the single entry point
every front end calls. It picks an output path (`name-compressed.pdf`, `-compressed-2` etc. if taken),
runs an engine into a same-directory temp file, and only renames it into place if the result is
actually smaller (`Outcome::NotSmaller` otherwise, nothing written).

- `level.rs`: `Level` (Lossless/Low/Medium/High) → `Profile` (target DPI, JPEG quality, trigger ratio).
  An image is only touched if its effective on-page DPI exceeds `target_dpi * trigger_ratio`.
- `engine.rs`: `Engine` trait (`compress(input, output, &Profile) -> Result<Report>`). Two impls:
  - `rust_engine.rs` (`RustEngine`, always available): `lopdf::Document::load` → `prune_objects` →
    `dedup_streams` (hashes stream bytes, merges byte-identical streams via `doc.traverse_objects`
    reference rewriting, then re-prunes) → `image_ops::resample_images` (skipped entirely for
    Lossless) → `doc.compress()` (Flate any uncompressed stream) → `doc.save_modern()` (xref +
    object streams) → reload and verify the page count didn't change (else the output is discarded
    and `PageCountMismatch` is returned).
  - `ghostscript_engine.rs` (`GhostscriptEngine`, optional): shells out to a Homebrew-installed `gs`
    (never bundled — its license is AGPL). Looks on `$PATH` first, then `/opt/homebrew/bin` and
    `/usr/local/bin`, since an app launched from Finder doesn't inherit a shell's PATH.
  - `EngineChoice::Best` runs both and keeps whichever output is smaller. Lossless always forces the
    Rust engine regardless of the caller's choice (Ghostscript's presets always resample images).
- `placement.rs`: figures out each image XObject's *effective DPI* by walking page (and nested Form
  XObject) content streams, tracking the CTM through `q`/`Q`/`cm`, and measuring the transformed unit
  square at each `Do`. When an image is drawn more than once, the smallest DPI (its most demanding
  placement) wins. Images the walk never reaches fall back to the document's largest page size
  (conservative — least likely to trigger unwanted resampling).
- `image_ops.rs`: only touches image kinds that round-trip safely — `DCTDecode` (JPEG) and raw
  8-bit-per-component DeviceGray/DeviceRGB (uncompressed or single-`FlateDecode`), including
  `ICCBased` colorspaces resolved via their stream's `/N`. Everything else (JBIG2, JPX, CCITT,
  indexed, CMYK, image masks, 16-bit…) is left untouched and counted as skipped. Resize via
  `image::imageops::resize` (Lanczos3), re-encode via `mozjpeg` (wrapped in `catch_unwind` per its
  own safety note), and only replace the object if the new bytes are actually smaller. An `SMask` is
  resized to match its parent's new dimensions and kept in Flate (never re-encoded to JPEG, to avoid
  alpha artifacts) via a scratch `lopdf::Stream::compress()` call that reuses lopdf's own
  "keep raw if compression doesn't help" logic.
- `config.rs`: `Config` (default level + engine) persisted at
  `~/Library/Application Support/com.haveneer.pdfshrinker/config.toml`. This is the single source of
  truth the CLI, the app and the Quick Action all read — the app's "set as default" checkbox writing
  here is what makes the (single, level-less) Quick Action follow the app's chosen default.
- `integration.rs`: installs the Finder Quick Action (fills in the `packaging/PdfShrinker.workflow`
  template via `include_str!` and writes it to `~/Library/Services`) and the `/usr/local/bin/pdfshrink`
  symlink. Shared verbatim by `pdfshrink-cli`'s `install` subcommand and the app's "Install
  integrations" button — `resolve_exec_path()` points both at the *bundled* CLI (`Contents/MacOS/pdfshrink`,
  the `externalBin` sidecar without its target-triple suffix) when running inside `PdfShrinker.app`, so
  they keep working across app updates instead of pinning today's exact process path.

### `crates/pdfshrink-cli`

`clap` derive; `pdfshrink [-l LEVEL] [-e ENGINE] [-j N] [--notify] FILES...` plus `config get/set` and
`install --quick-action --cli-link` subcommands. Files are compressed in parallel with `rayon`. Exit
codes: `0` success, `1` any error, `2` any file was already optimal (checked after all files, so a
mix of outcomes still surfaces the most severe code). `--notify` shells out to `osascript` — used by
the Quick Action so a background compression still tells the user something happened.

### `app/` — the GUI

Tauri 2, chosen specifically because plain egui/winit has no API for the `NSApplication` `odoc` Apple
Event macOS sends for "Open With" (would need patching the delegate via `objc2`); Tauri exposes it as
`RunEvent::Opened { urls }`, fired both at cold start (also handled manually in `setup()` from argv,
since the very first launch doesn't go through `RunEvent`) and while already running. `app/ui` is
static HTML/CSS/JS with **no npm/bundler** — `withGlobalTauri: true` in `tauri.conf.json` exists
specifically so `app.js` can call `window.__TAURI__.core.invoke(...)` / `.event.listen(...)` directly.
Rust commands in `app/src-tauri/src/lib.rs` (`compress_files`, `get_config`, `set_default_level`,
`set_default_engine`, `reveal_in_finder`, `install_integrations`) call straight into `pdfshrink-core`
— the app does not shell out to its own CLI sidecar for compression, only the Quick Action does that.
`compress_files` emits one `compress-result` event per file as it finishes rather than returning a
batch, so the file list updates incrementally.

### `packaging/PdfShrinker.workflow` — the Quick Action template

A hand-authored Automator "Service" bundle (`Info.plist` + `document.wflow`, an Automator "Run Shell
Script" action with `inputMethod=1` i.e. "as arguments", so the script sees selected Finder PDFs as
`"$@"`). `__PDFSHRINK_EXEC__` in `document.wflow` is filled in by `integration::install_quick_action`
at install time with the real path to the bundled CLI. **This template was hand-written and has not
been round-tripped through Automator.app itself** (no GUI available while building this) — the
individual pieces (build, packaging, code signing, file associations, launch-with-argv, `RunEvent::Opened`)
were each verified to work; the Quick Action XML specifically should get one manual check (install it,
right-click a PDF in Finder → Quick Actions → PdfShrinker) before relying on it.

### `scripts/build-dmg.sh`

Builds the CLI for `aarch64-apple-darwin`, stages it as the app's `externalBin` sidecar, then
`cargo tauri build --bundles dmg`. Ad-hoc signed only (`signingIdentity: "-"` in `tauri.conf.json`) —
Gatekeeper will warn on another Mac. The script's trailing comment block is where Developer ID signing
+ `notarytool`/`stapler` calls go once a signing identity is available.
