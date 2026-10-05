# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A macOS PDF compressor (Rust), à la iLovePDF/UPDF, ships three ways from one Cargo workspace:
CLI (`pdfshrink`), a Tauri GUI (`PdfShrinker.app`), and a Finder service — all going through the
same `pdfshrink-core` compression engine so behavior never drifts between front ends. Apple Silicon
only (`aarch64-apple-darwin`); no x86_64/universal build. Exception: `.github/workflows/windows.yml`
(manual `workflow_dispatch` on master) builds a Windows x86_64 CLI and app (static CRT, NSIS
installer). There, `app/src-tauri/tauri.windows.conf.json` drops the CLI sidecar and Tauri's
`fileAssociations` (its NSIS script would take over the default PDF handler);
`app/src-tauri/windows/pdf-open-with.nsh` registers the app as an extra "Open with" choice instead.
The UI hides "Install integrations" (macOS-only) on Windows.

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
cargo run -p pdfshrink-cli -- install --finder-service --cli-link   # writes to ~/Library/Services and /usr/local/bin (or ~/.local/bin) — real side effects, don't run from CI/agents without asking

# Generate a throwaway test PDF (oversized JPEG on one page) instead of needing a real file
cargo run -p pdfshrink-core --example make_fixture -- /path/to/out.pdf

# Generate PDFs covering every image kind (CMYK, JPEG 2000, CCITT, 1-bit, 16-bit, indexed…)
# into ./test-pdfs (gitignored); dev-only tools: imagemagick, openjpeg, img2pdf, qpdf, Pillow.
# See TODO.md for the results on these files.
scripts/make-test-pdfs.sh

# Diagnose a disappointing compression ratio: list every image XObject (id, size, filter,
# colorspace) and how many exceed 2000px on a side — run on input vs. output to see what
# actually got touched
cargo run --release -p pdfshrink-core --example inspect_images -- file.pdf
# Where do the bytes go (images/smasks/fonts/content…), how many images are duplicates once
# decoded; per-font embedded programs (spots un-merged per-page font subsets)
cargo run --release -p pdfshrink-core --example analyze -- file.pdf
cargo run --release -p pdfshrink-core --example fonts -- file.pdf
# Simple TrueType fonts (cmap, non-empty glyphs) / every FontFile3 program written to <dir>/*.cff
cargo run --release -p pdfshrink-core --example ttprobe -- file.pdf [BaseFontSubstring]
cargo run --release -p pdfshrink-core --example dump_fontfile3 -- file.pdf <dir>

# Levels: lossless, low, medium (default), high, extreme, extreme-max (same list in CLI and app).
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
runs the engine into a same-directory temp file, and only renames it into place if the result is
actually smaller (`Outcome::NotSmaller` otherwise, nothing written).

- `level.rs`: `Level` (Lossless/Low/Medium/High/Extreme/ExtremeMax) → `Profile` (target DPI, JPEG quality, trigger ratio, per-pass switches).
  An image is only touched if its effective on-page DPI exceeds `target_dpi * trigger_ratio`.
- `engine.rs`: `Engine` trait (`compress(input, output, &Profile) -> Result<Report>`), implemented by
  `rust_engine.rs` (`RustEngine`) — the only engine. There used to be an optional Ghostscript engine
  (and a "best of both" mode); it was removed because, on real documents, it never beat the Rust
  engine at equal quality, rotated some pages (`AutoRotatePages`), and required an external AGPL
  binary. The app is fully stand-alone. `RustEngine`: `lopdf::Document::load` → `prune_objects` →
    `dedup_streams` (hashes stream bytes, merges byte-identical streams via `doc.traverse_objects`
    reference rewriting, then re-prunes) → `image_ops::resample_images` (skipped entirely for
    Lossless) → `doc.compress()` (Flate any uncompressed stream) → `doc.save_modern()` (xref +
    object streams) → reload and verify the page count didn't change (else the output is discarded
    and `PageCountMismatch` is returned).
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
- `image_ops.rs`: decodes gray/RGB/CMYK (device or `ICCBased` via the ICC stream's `/N`), `Indexed`
  and 16-bit images stored as `DCTDecode` (also wrapped as `[/FlateDecode /DCTDecode]`, as iLovePDF
  writes them), `JPXDecode` or raw/`FlateDecode` samples; codecs live in `image_codecs.rs`
  (hayro-jpeg2000, mozjpeg CMYK, `fax` CCITT G4). CMYK is re-encoded as CMYK JPEG, never converted:
  an Adobe-marker CMYK JPEG source keeps its stored samples and `/Decode`, any other CMYK source is
  written inverted with `/Decode [1 0 1 0 1 0 1 0]`. A JPX alpha channel with `/SMaskInData` becomes
  a real `/SMask`. 1-bit raw/Flate/CCITT-G4 images and stencil masks take a separate path
  (`plan_bilevel`): downsampled to the colour targets × `BILEVEL_RESOLUTION_FACTOR` (2) with an
  ink-preserving threshold capped at 35 % coverage (`image_codecs::downsample_bilevel`), then CCITT
  G4 if smaller. JBIG2, CCITT G3, Lab, Separation/DeviceN… are left untouched and counted as
  skipped. Every eligible image
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
- Levels are one flat `Profile` struct per `Level` in `level.rs`, all built from `Profile::BASE`
  (lossless passes on, nothing geometry/look-changing); `Level::summary()` is the one-line
  description `--help` prints (the app has translated copies in `app/ui/i18n.js` — keep them in
  sync). Lossless passes (deep dedup, font merge, CFF) run at every level; gray/palette/SMask
  tricks at every lossy level; SSIM quality from `high` up; `page_px`, crop, `scan_whiten` and
  Zopfli only at `extreme`/`extreme-max`. Passes: `deep_dedup.rs` (merge objects equal once *decoded*, dictionary minus
  encoding keys, iterated to a fixpoint — two images become equal once their SMasks merged);
  `font_merge.rs` (union the per-page subsets of one `CIDFontType2`/Identity TrueType font —
  subsetters keep original GIDs — into one program, only when glyph data/hinting agree);
  `type1_merge.rs` (same for Type 1 `/FontFile` subsets — LaTeX figures each carry their own CMR10…:
  eexec-decrypt, union glyphs by name and `Subrs` by index, `return`-stub Subrs count as absent,
  compare decrypted charstrings, re-encrypt); `type1_cff.rs` (then converts every Type 1 program to
  CFF `/FontFile3` `/Subtype /Type1C`: charstrings interpreted to absolute outlines — subrs
  expanded, flex → curves, seac kept — and re-encoded as Type 2, built-in encoding preserved, stems
  merged into one non-overlapping set, hint replacement dropped; standard strings in
  `cff_tables.rs`, generated from fontTools); `cff_subset.rs` (completes sloppy CFF subsets:
  unreached subrs → `return` stubs without changing the bias, CID-keyed fonts lose unused strings
  and, when `.notdef` is blank, every blank glyph via a rebuilt charset; no charstring is rewritten);
  `zopfli_pass.rs` (re-deflate non-image Flate streams);
  and in `image_ops.rs`: page-relative resolution cap (`page_px`, pixels across the page's
  *displayed width* — DPI is meaningless for 1920×1080 pt slide pages), JPEG quality chosen per
  image by binary search on SSIM (luma, with the worst RGB channel + 0.03 as a chroma guard),
  scanned pages (an image spanning the page width, mostly light non-pure-white paper) get a fixed
  mid-range quality instead of the SSIM search and, with `scan_whiten`, a levels stretch mapping
  the paper tone to white, near-gray RGB → gray, ≤256-colour images kept lossless as `Indexed` + PNG predictor,
  opaque SMasks dropped, SMasks PNG-predicted, and cropping images to the part inside the page
  (`placement.rs`'s `visible`) via a Form XObject wrapper that takes over the image's id — only when
  the walk was `complete` and every reference to the image came from a walked `/XObject` dict.
- `config.rs`: `Config` (default level) persisted at
  `~/Library/Application Support/com.haveneer.pdfshrinker/config.toml`. This is the single source of
  truth the CLI, the app and the Finder service all read — the app's "set as default" checkbox writing
  here is what makes the (single, level-less) service follow the app's chosen default.
- `integration.rs`: installs the Finder service (fills in the `packaging/PdfShrinker.workflow`
  template via `include_str!` and writes it to `~/Library/Services`) and the `pdfshrink` symlink —
  `/usr/local/bin` when writable, else `~/.local/bin` (`/usr/local/bin` is `root:wheel` on a stock
  macOS, so the GUI app can never write there; failing outright is useless to the user). Shared verbatim by `pdfshrink-cli`'s `install` subcommand and the app's "Install
  integrations" button — `resolve_exec_path()` points both at the *bundled* CLI (`Contents/MacOS/pdfshrink`,
  the `externalBin` sidecar without its target-triple suffix) when running inside `PdfShrinker.app`, so
  they keep working across app updates instead of pinning today's exact process path.

### `crates/pdfshrink-cli`

`clap` derive; `pdfshrink [-l LEVEL] [--tune K=V] [--suffix S] [-j N] [--notify] FILES...` plus `config get/set` and
`install --finder-service --cli-link` subcommands (`--quick-action` kept as an alias). Files are compressed in parallel with `rayon`. Exit
codes: `0` success, `1` any error, `2` any file was already optimal (checked after all files, so a
mix of outcomes still surfaces the most severe code). `--notify` shells out to `osascript` — used by
the Finder service so a background compression still tells the user something happened.

### `app/` — the GUI

Tauri 2, chosen specifically because plain egui/winit has no API for the `NSApplication` `odoc` Apple
Event macOS sends for "Open With" (would need patching the delegate via `objc2`); Tauri exposes it as
`RunEvent::Opened { urls }`, fired both at cold start (also handled manually in `setup()` from argv,
since the very first launch doesn't go through `RunEvent`) and while already running. `app/ui` is
static HTML/CSS/JS with **no npm/bundler** — `withGlobalTauri: true` in `tauri.conf.json` exists
specifically so `app.js` can call `window.__TAURI__.core.invoke(...)` / `.event.listen(...)` directly.
Rust commands in `app/src-tauri/src/lib.rs` (`compress_files`, `get_config`, `set_default_level`,
`reveal_in_finder`, `install_integrations`) call straight into `pdfshrink-core`
— the app does not shell out to its own CLI sidecar for compression, only the Finder service does that.
`compress_files` is `async` and runs the batch in `spawn_blocking` (a sync command would run on the
main thread and freeze the webview — no repaint, no scrolling); it emits `compress-started` then
`compress-result` per file rather than returning a batch, so each one-line row in the file list
shows pending → queued → running → done/error as it happens. "Compress all" runs every pending or
failed file; a row's status button (re)runs just that file.

### `packaging/PdfShrinker.workflow` — the Finder service template

A hand-authored Automator "Service" bundle (`Info.plist` + `document.wflow`, an Automator "Run Shell
Script" action with `inputMethod=1` i.e. "as arguments", so the script sees selected Finder PDFs as
`"$@"`). `__PDFSHRINK_EXEC__` in `document.wflow` is filled in by `integration::install_finder_service`
at install time with the real path to the bundled CLI. It was hand-written rather than round-tripped
through Automator.app (no GUI while building this), so its `workflowMetaData` was aligned key-for-key
against a known-good Automator-produced workflow; verified end to end with
`automator -i file.pdf ~/Library/Services/PdfShrinker.workflow`, which is the way to test it without
clicking through Finder.

**It lands in the contextual menu's *Services* submenu, never "Quick Actions".** As of macOS 26 that
submenu — and the Finder list in System Settings → General → Login Items & Extensions — is fed only by
Action extensions (`.appex` with `NSExtensionPointIdentifier` `com.apple.ui-services`, the way
ImageOptim does it) and by Shortcuts; no Automator `.workflow` appears in either, whoever wrote it.
A real "Quick Actions" entry would mean shipping an `.appex` in `Contents/PlugIns/` — a separate
Swift/AppKit target outside cargo, built and signed by `scripts/build-dmg.sh`. Deliberately not done;
that's why everything the user reads says "Finder service". Useful probes when this misbehaves:
`/System/Library/CoreServices/pbs -dump_pboard | grep -A12 pdfshrinker` (is it registered at all?),
`/System/Library/CoreServices/pbs -update -flush`, and `defaults read pbs` (`FinderActive` lists the
`.appex`/Shortcuts quick actions, and only those).

### `scripts/build-dmg.sh`

Builds the CLI for `aarch64-apple-darwin`, stages it as the app's `externalBin` sidecar, then
`cargo tauri build --bundles dmg`. Ad-hoc signed only (`signingIdentity: "-"` in `tauri.conf.json`) —
Gatekeeper will warn on another Mac. The script's trailing comment block is where Developer ID signing
+ `notarytool`/`stapler` calls go once a signing identity is available.
