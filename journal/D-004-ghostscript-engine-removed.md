# D-004 — The optional Ghostscript engine and its "best of both" mode are removed

| | |
|---|---|
| **Identifier** | D-004 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-09-30) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (Ghostscript engine, "best of both"), `b067d51` (removal) |

## 1. Context

The first version shipped two engines behind the `Engine` trait: the Rust engine (D-003) and an
optional one shelling out to a Homebrew-installed Ghostscript (`-dPDFSETTINGS=/screen`, images at
96 dpi), plus a "best of both" mode running both and keeping the smaller file. Ghostscript was the
obvious reference: it re-encodes everything, including image kinds the Rust engine did not handle
yet (CMYK, JPEG 2000, JBIG2, CCITT), and repairs some broken files.

## 2. Options considered

**A** — Keep both engines and "best of both" as the default, so users always get the smaller file.

**B** — Keep Ghostscript as an opt-in engine only, for the cases the Rust engine skipped.

**C** — Remove it, and put what it could still do on the Rust engine's to-do list.

## 3. What the assistant proposed

**C**, on the measurements of `docs/compression-strategies.md` §4.4 and with the reasoning in the
commit message of `b067d51`: "on the sample documents it never beat the Rust engine at equal quality,
its default AutoRotatePages added a /Rotate to some pages, 'best' silently picked lower-quality
output, and it required an external AGPL binary."

## 4. What we decided, and why

**C**. On seven real documents, Ghostscript was never both smaller and closer to the original than the
Rust engine's `extreme` (page SSIM against the original, worst page in parentheses):

* where it beat the Rust engine of the time on size (LaTeX thesis 5.7 MB vs 9.0 MB, business proposal
  0.54 vs 1.4 MB), it did so at a visible cost (worst page 0.797 and 0.870) — and "best of both",
  choosing by size alone, would have picked those files silently;
* it **changed pages**: its default `AutoRotatePages` added a `/Rotate` to one slide and two thesis
  pages, which breaks D-005's promise;
* it made the app depend on an external AGPL binary the user had to install.

The removal took `ghostscript_engine.rs`, `EngineChoice`, the `-e/--engine` flag, the `engine` config
key (older config files still load, D-014) and the app's selector. Its remaining advantages became
`TODO.md` items, and most were done the same day in Rust (CMYK, JPEG 2000, 16-bit, palettes, 1-bit,
`fe6a6d2` and `31ea3cd`).

## 5. What would make us change our minds

* If repairing broken PDFs became frequent enough to matter: that is the one thing Ghostscript still
  does that we do not (`TODO.md`). Even then, D-003's narrower option (a repair-only step) comes first.
* "Best of both" is wrong in principle as long as it chooses by size alone: any future multi-engine
  mode needs a quality criterion (D-020 provides one for images).

## Open points (review of 2026-11-20)

* The `Engine` trait (`engine.rs`) now has a single implementation, `RustEngine`, and `compress_file`
  calls it directly. It no longer abstracts anything; keep it as a seam for tests or a future
  engine, or fold it into `rust_engine.rs`.
