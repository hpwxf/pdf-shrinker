# D-021 — Tests: fixtures synthesized in code, generated test PDFs, a private corpus outside the repository

| | |
|---|---|
| **Identifier** | D-021 |
| **Date** | 2026-11-20 (retrospective; decisions taken from 2026-09-28 to 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (`tests/compression.rs`, `make_fixture`), `7d41753` (`analyze`, `fonts`, comparison documents), `b067d51` (`scripts/make-test-pdfs.sh`), `fe6a6d2` (seven integration tests), `247e56d` (`ttprobe`, `dump_fontfile3`) |

## 1. Context

What can go wrong in a PDF compressor is mostly invisible to unit tests: a page that renders slightly
differently, a font that loses a glyph, a file that is valid but bigger. Real PDFs are the only
convincing inputs, but they are large and often private (bank statements, contracts, students' exam
papers in `docs/compression-strategies.md` §4.3). Nothing binary should be committed.

## 2. Options considered

**A** — Commit a set of real PDFs as fixtures and render-compare in CI. The strongest check; private
documents in a public repository, and a heavy CI.

**B** — Layers: unit and integration tests on PDFs **built in code**; a script generating PDFs per
image kind with dev-only tools; a corpus of real documents and render comparisons run by hand, outside
the repository, whose results are written into commits and docs.

## 3. What the assistant proposed

**B**; not recorded as a single choice, it emerged commit by commit. Every engine commit reports its
effect on the reference documents or the corpus, and the validations it relied on (fontTools,
poppler, Ghostscript renders, `qpdf --check`).

## 4. What we decided, and why

**B**:

* **In the repository, run by CI**: ~40 unit tests in the core modules and 21 integration tests in
  `crates/pdfshrink-core/tests/compression.rs`, all on fixtures synthesized in the test (lopdf
  documents, `image`-written JPEGs, a 279-byte embedded JPEG 2000 codestream). They check behaviour
  (downsampled or not, SMask resized, dedup, crop and its safeguards, encrypted rejected, not-smaller
  reported, G4 pixel-exact round trip), not sizes.
* **Generated, gitignored**: `scripts/make-test-pdfs.sh` writes one PDF per image kind (CMYK, ICC
  CMYK, JPEG 2000, indexed, 16-bit, 1-bit, CCITT) into `./test-pdfs` with ImageMagick, OpenJPEG,
  img2pdf (embeds without re-encoding, so each file has exactly the targeted filter) and qpdf. Results
  in `TODO.md`. `make_fixture` gives a throwaway oversized-JPEG PDF; `web/smoke-test.mjs` uses it.
* **Outside the repository, by hand**: the three comparison documents of
  `docs/compression-strategies.md` §4 (slide deck, LaTeX thesis, scanned exam) and a corpus of ~225
  real files; page SSIM with a Python script (`pdftoppm -r 40 -gray` + scikit-image); font checks with
  fontTools; pixel-identical renders with poppler and Ghostscript. The diagnostics that make this
  practical are examples in the core: `analyze`, `inspect_images`, `fonts`, `ttprobe`,
  `dump_fontfile3`.
* Validation of a compressed file: `qpdf --check` structurally, `pdftoppm -png` visually.

## 5. What would make us change our minds

* A regression that the in-repository tests missed and the corpus would have caught: the case for a
  small committed set of **non-private** real-world PDFs (generated slides, a LaTeX document built in
  CI) with size and render thresholds.
* The out-of-repository scripts (SSIM, fontTools checks) are a single point of failure: they exist on
  one machine. If they matter for every engine change, they belong under `scripts/`.

## Open points (review of 2026-11-20)

* Determinism (D-012) and "no file bigger at any level" (D-005) are corpus findings with no test.
* The SSIM script behind every number in `docs/compression-strategies.md` is not in the repository, so
  the measurements cannot be reproduced from it (§6 says so).
