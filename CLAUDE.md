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

# Diagnose a disappointing compression ratio: list every image XObject (id, size, filter,
# colorspace) and how many exceed 2000px on a side — run on input vs. output to see what
# actually got touched
cargo run --release -p pdfshrink-core --example inspect_images -- file.pdf
# Where do the bytes go (images/smasks/fonts/content…), how many images are duplicates once
# decoded; per-font embedded programs (spots un-merged per-page font subsets)
cargo run --release -p pdfshrink-core --example analyze -- file.pdf
cargo run --release -p pdfshrink-core --example fonts -- file.pdf

# Experimental levels (CLI only, not offered by the app): extreme-safe, extreme, extreme-max.
# --tune overrides one profile knob (repeatable), --suffix names the output <name>-<suffix>.pdf,
# PDFSHRINK_DEBUG=1 prints per-phase timings and the JPEG quality search
cargo run --release -p pdfshrink-cli -- -l extreme --tune page_px=1800 --suffix x1800 file.pdf

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
  (conservative — least likely to trigger unwanted resampling). The walk's op budget is per page.
  Images are planned in parallel (rayon) from cloned streams, then applied sequentially. lopdf only
  honours a *direct* `/DecodeParms` dict, so `image_ops` inlines indirect ones before decoding. This CTM-based size is not reliable
  for an image drawn oversized and then clipped to the visible page area (a common "full-bleed
  background" export from slide tools) — it overstates the on-page footprint and so understates the
  DPI, which `image_ops.rs`'s `max_dimension` cap exists specifically to catch.
- `image_ops.rs`: only touches image kinds that round-trip safely — `DCTDecode` (JPEG, also when
  wrapped as `[/FlateDecode /DCTDecode]`, as iLovePDF writes them) and raw 8-bit-per-component DeviceGray/DeviceRGB (uncompressed or single-`FlateDecode`), including
  `ICCBased` colorspaces resolved via their stream's `/N`. Everything else (JBIG2, JPX, CCITT,
  indexed, CMYK, image masks, 16-bit…) is left untouched and counted as skipped. Every eligible image
  is re-encoded as JPEG at the profile's `jpeg_quality` regardless of resolution (a raw/Flate bitmap
  shrinks a lot from that alone); on top of that, it's downsampled (`image::imageops::resize`,
  Lanczos3) if either its placement-derived DPI exceeds `target_dpi * trigger_ratio` *or* its longest
  side exceeds the profile's `max_dimension` — whichever wants the smaller result wins. Re-encode via
  `mozjpeg` (wrapped in `catch_unwind` per its own safety note), and only replace the object if the
  new bytes are actually smaller. An `SMask` is resized to match its parent's new dimensions and kept
  in Flate (never re-encoded to JPEG, to avoid alpha artifacts) via a scratch `lopdf::Stream::compress()`
  call that reuses lopdf's own "keep raw if compression doesn't help" logic.
- `build.rs` / `version.rs`: bakes the git commit (and dirty-tree flag) into the binary via
  `cargo:rustc-env` + `env!()`, exposed at runtime as `pdfshrink_core::build_info()`. This lives in
  `pdfshrink-core` alone, not in each front end — `env!()` only resolves within the crate that writes
  the env var, but a plain function call works across crates, so the CLI (`--version`) and the app
  (footer "ⓘ" tooltip) both just call it instead of each needing their own `build.rs`.
- Experimental `Extreme*` levels (`Level::EXPERIMENTAL`, deliberately not in `Level::ALL` so the
  GUI doesn't list them) switch on the `Experimental` knobs in `level.rs`; regular levels use
  `Experimental::OFF`. Passes: `deep_dedup.rs` (merge objects equal once *decoded*, dictionary minus
  encoding keys, iterated to a fixpoint — two images become equal once their SMasks merged);
  `font_merge.rs` (union the per-page subsets of one `CIDFontType2`/Identity TrueType font —
  subsetters keep original GIDs — into one program, only when glyph data/hinting agree);
  `type1_merge.rs` (same for Type 1 `/FontFile` subsets — LaTeX figures each carry their own CMR10…:
  eexec-decrypt, union glyphs by name and `Subrs` by index, `return`-stub Subrs count as absent,
  compare decrypted charstrings, re-encrypt); `type1_cff.rs` (then converts every Type 1 program to
  CFF `/FontFile3` `/Subtype /Type1C`: charstrings interpreted to absolute outlines — subrs
  expanded, flex → curves, seac kept — and re-encoded as Type 2, built-in encoding preserved, stems
  merged into one non-overlapping set, hint replacement dropped; standard strings in
  `cff_tables.rs`, generated from fontTools); `zopfli_pass.rs` (re-deflate non-image Flate streams);
  and in `image_ops.rs`: page-relative resolution cap (`page_px`, pixels across the page's
  *displayed width* — DPI is meaningless for 1920×1080 pt slide pages), JPEG quality chosen per
  image by binary search on SSIM (luma, with the worst RGB channel + 0.03 as a chroma guard),
  scanned pages (an image spanning the page width, mostly light non-pure-white paper) get a fixed
  mid-range quality instead of the SSIM search and, with `scan_whiten`, a levels stretch mapping
  the paper tone to white, near-gray RGB → gray, ≤256-colour images kept lossless as `Indexed` + PNG predictor,
  opaque SMasks dropped, SMasks PNG-predicted, and cropping images to the part inside the page
  (`placement.rs`'s `visible`) via a Form XObject wrapper that takes over the image's id — only when
  the walk was `complete` and every reference to the image came from a walked `/XObject` dict.
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
