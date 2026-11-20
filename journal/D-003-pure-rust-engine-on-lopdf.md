# D-003 — A pure-Rust engine on `lopdf`, every compression pass written in-house

| | |
|---|---|
| **Identifier** | D-003 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-09-28) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (`RustEngine` on lopdf), `7d41753` (deep dedup, font passes, Zopfli), `fe6a6d2` (`image_codecs.rs`), `247e56d` (`cff_subset.rs`) |

## 1. Context

A PDF compressor needs to parse and rewrite the object graph, decode and re-encode images, and — as it
turned out — rewrite font programs. The language is Rust (the project's default). The question is how
much of the PDF and codec work to delegate: to a mature C/C++ library through bindings, to an external
tool, or to Rust crates plus our own code.

## 2. Options considered

**A** — Bind a mature C/C++ PDF library (qpdf, MuPDF, pdfium). Robust parsing and repair of broken
files for free; a C toolchain in every build, a WebAssembly build made much harder, and MuPDF is AGPL.

**B** — Shell out to an external tool (Ghostscript, qpdf) for part of the work. The initial commit did
this optionally for Ghostscript (D-004).

**C** — `lopdf` for the object model (load, traverse, save with object streams), Rust crates for codecs
(`image` for resampling, `flate2`, `zopfli`, `hayro-jpeg2000`, `fax`, `jpeg-encoder`/`jpeg-decoder`,
mozjpeg as the one C dependency, D-019), and every compression-specific pass written in-house.

## 3. What the assistant proposed

**C** from the first commit, with Ghostscript as an *optional* second engine. Not recorded at the time
why lopdf rather than a binding; the choice of pure-Rust codecs later is argued in the commits
(`fe6a6d2`: hayro-jpeg2000 "default features off, so no moxcms/image pull-in"; `31ea3cd`: "pure-Rust
`fax`").

## 4. What we decided, and why

**C**. The engine is ~300 KB of Rust in `crates/pdfshrink-core/src`, of which the biggest parts are
exactly what no library offers: `image_ops.rs` (placement-aware image re-encoding), `font_merge.rs`,
`type1_merge.rs`, `type1_cff.rs`, `cff_subset.rs`.

Why it held:

* **Control over every byte.** The passes that beat iLovePDF (deep dedup on decoded content, subset
  merging, Type 1 → CFF, palettes, cropping) need to look inside objects in ways a general PDF
  library's API does not expose. Ghostscript, the one external engine tried, rebuilt the file its own
  way and lost on quality (D-004).
* **No runtime dependency.** The app is stand-alone, no Homebrew, no AGPL binary.
* **WebAssembly for free, almost.** With mozjpeg behind a feature, the whole core builds for
  `wasm32-unknown-unknown` (D-018); a C PDF library would have made this a project of its own.

The price, paid knowingly:

* `lopdf` limits what can be read. It only honours a *direct* `/DecodeParms` (`image_ops` inlines
  indirect ones; before that fix, the LaTeX thesis lost 4 MB of savings, `7d41753`), and files it
  refuses to open cannot be repaired (`TODO.md`).
* Every codec gap is ours to fill: JBIG2 and CCITT G3 are still left untouched (D-010).

## 5. What would make us change our minds

* If repairing broken files became a requirement (users sending PDFs `lopdf` rejects): a binding
  behind a `native`-only feature, used only to rewrite the file before our passes, would be the
  narrowest way back to A.
* If `lopdf` stopped being maintained or blocked a format feature we need (e.g. incremental
  updates, encryption, PDF 2.0 specifics).
