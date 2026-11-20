# Changelog

All notable changes to PdfShrinker: the `pdfshrink` CLI, the `PdfShrinker.app` desktop app, the
Finder service and the web version, which share one compression engine (`pdfshrink-core`) and one
version number. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning](https://semver.org/). Decision-journal entries (`journal/`)
are cited as D-NNN.

## [Unreleased]

## [0.3.0] - 2026-11-20

### Added

- **Flatten annotations** (`--flatten`, a "Flatten annotations" checkbox in the app and on the
  web page). Text boxes, drawings, stamps, signatures, highlights and filled form fields are
  drawn into their page and removed, so they can no longer be moved or edited. Nothing is
  rasterized: text stays selectable and searchable. Links, sticky notes, attachments and
  multimedia are kept. A flattened file is written even when it isn't smaller. CLI, app and web
  show how many annotations were flattened (D-027).
- `scripts/make-annotated-pdfs.sh`: generates annotated test PDFs with PDFKit, the framework
  behind macOS Preview (macOS only, nothing committed).
- Decision journal (`journal/`): architecture decision records D-001 to D-027.
- License: `MIT OR Apache-2.0`, with `LICENSE-MIT` and `LICENSE-APACHE` (D-025).

### Fixed

- PDFs annotated or filled in macOS Preview (or another PDFKit-based app) lost the fonts of their
  annotations at every level, `lossless` included: their font streams have no `/Length`, and the
  PDF library loaded them empty. They are now read back from the file. A stream still unreadable
  after that makes the file refused instead of written degraded (D-026).
- Merged TrueType font subsets could draw a letter or a box in place of every space (Preview
  annotations, Pages documents, Keynote decks) (D-009).
- A diagonal seam through images whose uncompressed soft mask carried a PNG predictor (jsPDF).
- Tagged PDFs no longer lose their structure tree.

### Changed

- lopdf 0.45 → 0.47: faster loading, more damaged files recovered.
- The CLI shows the size change with its sign (`+2%` for a flattened file that grew).
- CI runs on macOS 15 (macOS 14 runners are no longer served).

## [0.2.0] - 2026-10-05

### Added

- **Web version**: the same engine compiled to WebAssembly runs entirely in the browser, deployed
  to GitHub Pages on demand; nothing is uploaded. Shows its version and JPEG encoder, and warns
  when a level's output can differ from the desktop's (D-018).
- Levels `extreme` and `extreme-max`: page-relative resolution cap for slide decks, images cropped
  to the page, scanned paper whitened, Zopfli (D-007, D-011).
- Image kinds: CMYK (kept CMYK), JPEG 2000, CCITT, 1-bit (downsampled, re-encoded CCITT G4),
  16-bit and palette images; near-gray images stored as gray, few-colour images kept lossless as
  a palette (D-010).
- JPEG quality chosen per image by SSIM search at every lossy level; fixed mid-range quality for
  scanned pages (D-008).
- Fonts: per-page subsets of one TrueType or Type 1 font merged into one program, Type 1 converted
  to CFF, sloppy CFF subsets completed (D-009).
- App: "Compare all levels" side by side with an image fidelity score; "Run in parallel";
  one-line file rows with per-file status, "Compress all" and per-file re-run; English and French
  (D-024).
- Build version and commit shown by `pdfshrink --version` and in the app's footer (D-023).
- Manual GitHub Actions builds: macOS `.app`/`.dmg`, Windows x86_64 CLI and app (D-016, D-017).
- CI: format, lint and tests on every push (D-022).

### Changed

- Levels normalized into one flat profile each, built from a shared base (D-007). `low` and
  `medium` downsample sooner and search the JPEG quality per image.
- Output is byte-identical from one run to the next (D-012).
- The Finder integration is a service in the *Services* submenu; the CLI symlink falls back to
  `~/.local/bin` when `/usr/local/bin` isn't writable (D-015).

### Removed

- The optional Ghostscript engine and its "best of both" mode: never better at equal quality,
  rotated some pages, and needed an external AGPL binary (D-004).

### Fixed

- Weak compression of images drawn oversized and clipped to the page (D-006).
- Soft masks with `/Matte` no longer change their parent image's colour space.
- App: files opened before the window was ready are no longer lost.

## [0.1.0] - 2026-09-28

### Added

- First version: a pure-Rust compression engine (`pdfshrink-core`, on lopdf) with dead-object
  pruning, stream deduplication, JPEG resampling to the images' effective on-page DPI (soft masks
  resized along) and Flate recompression; four levels (`lossless`, `low`, `medium`, `high`).
- `pdfshrink` CLI: parallel compression of several files, `config get/set`, `install`.
- `PdfShrinker.app` (Tauri 2) with Finder "Open With" support, and a Finder service calling the
  bundled CLI.
- One configuration file for the default level, shared by the CLI, the app and the service
  (D-014).

[Unreleased]: https://github.com/hpwxf/pdf-shrinker/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/hpwxf/pdf-shrinker/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/hpwxf/pdf-shrinker/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hpwxf/pdf-shrinker/releases/tag/v0.1.0
