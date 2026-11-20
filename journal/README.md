# Decision journal — `pdf-shrinker`

A log of ADRs (*Architecture Decision Records*): one file per architectural trade-off, with the options
that were set aside and what would make us change our minds. Format taken from the sibling projects
`~/haveneer/slidekit/journal` and `~/haveneer/Alambic/journal`; unlike them, this journal is written in
**English**, like the rest of this repository (D-001).

## What goes in here

What **the goal and the PDF specification do not fix**. The goal is "shrink a PDF as much as
iLovePDF or better, stay a valid PDF, never visibly damage the page at the default level"; the
specification (ISO 32000) says what a valid file is, what a font program or an image filter may look
like. Neither says which language or PDF library to use, how many front ends there are and how they
share code, what a compression level is, how image quality is chosen, which passes are allowed to
change what a page looks like, or how the result is tested and shipped. That is the scope of this
journal.

A property of the format is not a decision: "a `/Type1` font dictionary accepts a `/FontFile3`
`Type1C` program" is a reading of the specification. Converting every Type 1 program to CFF at every
level, and dropping hint replacement to do it, is a decision (D-009). A measurement (the slide deck
at `extreme` is 7.6 MB) goes into `docs/compression-strategies.md`, not here; an entry cites it.

## Format

One file per decision, `D-NNN-english-slug.md`. Header: identifier, date, author, status (`proposed` /
`adopted` / `revised` / `abandoned`), related commits. Then five fields, all mandatory:

1. **Context** — the question that came up;
2. **Options considered** — at least two, with what sets them apart;
3. **What the assistant proposed** — faithfully, including when it was the right answer;
4. **What we decided, and why**;
5. **What would make us change our minds** — the signal that would say it was a mistake.

A `proposed` decision has fields 1 to 3 filled in; field 4 says what is left to settle and how
(measurement on the corpus, a test, a question to the user). It becomes `adopted` in the commit that
settles it.

An entry may end with an **"Open points (review of YYYY-MM-DD)"** section: a doubt found on rereading,
not settled yet. It does not change the status; the decision that comes out of it, if it differs, is
recorded as a revision (dated section, `Status: adopted, revised`).

## Retrospective entries: D-002 to D-025

The journal was opened on 2026-11-20, after 35 commits (2026-09-28 to 2026-10-05, version 0.2.0).
Entries **D-002 to D-025 are reconstructed** from `git log` (most commit messages argue their
choice), `CLAUDE.md`, `README.md`, `TODO.md`, `docs/compression-strategies.md` and the code. They carry
`Status: adopted (retrospective)`. Two consequences, owned rather than hidden:

* field 3, "what the assistant proposed", is **partly unknowable**. Every commit is co-authored by a
  Claude model (Sonnet 5 for the first ones, then Opus 5.5, Opus 5 and Sonnet 5.5), but the
  conversations are not kept. Where a commit message records the reasoning, the entry quotes or
  paraphrases it and says so; where the trace says nothing, the entry writes "not recorded at the
  time" instead of reconstructing a plausible dialogue;
* `git log --diff-filter=A -- journal/` shows these entries added **in a single commit**. That is
  the truth: the journal starts there. The progression this command checks only holds from D-026 on.

## Precedence rule

From D-026 on, an entry is committed **before or with** the code it describes, as the first file of
that commit.

```bash
git log --diff-filter=A -- journal/
```

If the implementation makes a recorded decision drift, the entry is **revised** (`Status: adopted,
revised`, plus a dated section at the end of the file) rather than a new one opened. Numbers are
never reassigned.

## Index

| ID | Decision | Status |
|---|---|---|
| [D-001](D-001-repository-conventions.md) | repository conventions: `journal/`, `CLAUDE.md` as the agents' reference, English everywhere, definition of "done" | adopted |
| [D-002](D-002-single-core-many-front-ends.md) | one compression core `pdfshrink-core`, called in-process by every front end (CLI, app, Finder service via the CLI, web) | adopted (retrospective) |
| [D-003](D-003-pure-rust-engine-on-lopdf.md) | a pure-Rust engine on `lopdf`, every pass written in-house; no qpdf/MuPDF/pdfium binding | adopted (retrospective) |
| [D-004](D-004-ghostscript-engine-removed.md) | the optional Ghostscript engine and its "best of both" mode removed after measurement | adopted (retrospective) |
| [D-005](D-005-never-worse-than-the-input.md) | never worse than the input: original untouched, temp file + rename, written only if smaller, page count checked, encrypted files refused | adopted (retrospective) |
| [D-006](D-006-effective-dpi-and-caps.md) | image resolution from the on-page placement (CTM walk), backed by a longest-side cap and, at `extreme*`, a page-relative cap; always re-encode | adopted (retrospective) |
| [D-007](D-007-six-flat-levels.md) | six cumulative levels, each one flat `Profile` built from `Profile::BASE`; lossless passes at every level; `--tune` overrides | adopted (retrospective), revised |
| [D-008](D-008-ssim-driven-jpeg-quality.md) | JPEG quality searched per image on SSIM (luma, chroma guard); fixed mid-range quality for detected scans | adopted (retrospective), revised |
| [D-009](D-009-font-program-rewriting.md) | font programs rewritten in-house: subset merging (CID TrueType, Type 1, simple TrueType), Type 1 → CFF, CFF subset completion; merge only when subsets agree | adopted (retrospective), revised |
| [D-010](D-010-image-kinds-and-colour.md) | image kinds: CMYK stays CMYK, JPEG 2000 and CCITT decoded in pure Rust, bi-level at 2× the colour targets, palettes lossless, soft masks always lossless | adopted (retrospective) |
| [D-011](D-011-look-changing-passes-only-at-extreme.md) | up to `high` a page never changes beyond compression artifacts; cropping, paper whitening, Zopfli and the page-relative cap only at `extreme*` | adopted (retrospective) |
| [D-012](D-012-deterministic-output.md) | byte-identical output from one run to the next (ordered maps where object numbers are assigned) | adopted (retrospective) |
| [D-013](D-013-tauri-gui-vanilla-js.md) | GUI on Tauri 2 (for macOS `odoc` events), static HTML/JS with no npm, core called directly, per-file events | adopted (retrospective) |
| [D-014](D-014-shared-config-file.md) | one persisted config file (`default_level`) read by CLI, app and Finder service; unknown keys ignored | adopted (retrospective) |
| [D-015](D-015-finder-service-automator-workflow.md) | Finder integration = Automator `.workflow` service calling the bundled CLI, not an `.appex` Quick Action; CLI symlink falls back to `~/.local/bin` | adopted (retrospective) |
| [D-016](D-016-platform-scope.md) | platform scope: macOS on Apple Silicon first (CI), Windows x86_64 on demand, any browser through the web version | adopted (retrospective), revised |
| [D-017](D-017-distribution-and-signing.md) | distribution: `.dmg` from `scripts/build-dmg.sh`, ad-hoc signing, manual release workflows; notarization deferred | adopted (retrospective) |
| [D-018](D-018-web-version-in-the-browser.md) | web version: the same core in WebAssembly, in the visitor's browser, static site on GitHub Pages; `native` feature gates desktop-only code | adopted (retrospective) |
| [D-019](D-019-jpeg-codecs.md) | JPEG: mozjpeg natively, pure-Rust `jpeg-encoder` otherwise; every JPEG decoded by `jpeg-decoder`, never zune-jpeg | adopted (retrospective) |
| [D-020](D-020-image-fidelity-at-144-dpi.md) | image fidelity reported as luma SSIM judged at 144 dpi on the page, area-weighted; always on in the app | adopted (retrospective) |
| [D-021](D-021-test-strategy.md) | tests: fixtures synthesized in-test, generated test PDFs, a private real-document corpus and render comparisons outside the repository | adopted (retrospective) |
| [D-022](D-022-ci-and-lint-policy.md) | CI: macOS arm64, `fmt --check`, `clippy -D warnings`, `test`, all `--locked`; builds and deploys manual-only; stable toolchain, not pinned | adopted (retrospective) |
| [D-023](D-023-build-identifier-in-core.md) | build identifier (commit + dirty flag) baked into `pdfshrink-core`, read by every front end through `build_info()` | adopted (retrospective) |
| [D-024](D-024-ui-languages.md) | app and web in English and French (`i18n.js`, shared); CLI in English only | adopted (retrospective) |
| [D-025](D-025-license.md) | license MIT at first, never written down; now `MIT OR Apache-2.0`, with `LICENSE-MIT` and `LICENSE-APACHE` | adopted (retrospective), revised |
| [D-026](D-026-streams-without-length.md) | streams without `/Length`: read back in the core after loading, fixed upstream in lopdf (fork branch), a stream still lost makes the file refused; lopdf 0.47 | adopted |

## Questions waiting for an entry

Collected from `TODO.md`, `docs/compression-strategies.md` §5 and the open points of the entries
above. Each one becomes a `proposed` entry when work on it starts, not before.

* **Font merging, next step**: merging CFF subsets (`FontFile3`), and TrueType subsets with renumbered
  GIDs (Skia/Chrome) through a per-font `CIDToGIDMap`. The remaining font gap with iLovePDF (D-009).
* **Bi-level and scans**: JBIG2 generic-region encoding; a bi-level mode for colourless scanned pages;
  a mixed-raster split (text mask + low-resolution background) (D-010).
* **CMYK → RGB** at `extreme*` for screen-only documents: needs a colour-management dependency
  (`lcms2`, C) or a naive formula, against D-003 and D-010.
* **Content-stream rewriting** (number rounding, useless operators): the first pass that would touch
  page content (D-005, D-011).
* **Lossy soft masks** for shadows and gradients (D-010).
* **Repairing broken PDFs** `lopdf` refuses to open: what Ghostscript used to do (D-004).
* **Developer ID signing and notarization**, and with them a real Finder Quick Action (`.appex`)
  (D-015, D-017).
