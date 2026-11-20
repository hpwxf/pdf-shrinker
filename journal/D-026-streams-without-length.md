# D-026 — Streams without `/Length`: recovered in the core, fixed upstream in lopdf, refused when lost

| | |
|---|---|
| **Identifier** | D-026 |
| **Date** | 2026-11-20 |
| **Author** | Pascal Havé |
| **Status** | adopted, revised |
| **Commits** | this entry's commit (`stream_recovery.rs`, lopdf 0.45 → 0.47, D-009 revision) |

## 1. Context

A one-page form produced by 4-Heights PDF Library 3.4 lost the weight of part of its text once
compressed, at every level including `lossless`. Its 32 form-field TrueType fonts store their
`/FontFile2` with `/Length1` but **no `/Length`**. lopdf's parser then defers the stream
(`Stream::with_position`), the deferred read fails for want of a length, and the error is ignored
(`let _ = self.read_stream_content(…)`): the stream loads **empty**. We wrote it back empty, the
viewer substituted a regular Helvetica for the embedded font. A lost content stream would have
blanked a page the same way. lopdf 0.45 to 0.47 (and its `main` on 2026-11-20) behave the same; since
0.45 they recover a *wrong* `/Length` (#568: the only EOL-framed `endstream` followed by `endobj`,
bounded by the next cross-reference offset), but not a *missing* one.

The recovered fonts then went through the font passes for the first time, which exposed holes in the
simple TrueType merge (an `X` drawn for every space); those are recorded as a revision of D-009.

## 2. Options considered

**A** — Workaround in `pdfshrink-core` only: after loading, read such streams back from the input
bytes. Immediate; code we own until lopdf changes.

**B** — Fix lopdf upstream and wait for a release. Right place for the fix (every lopdf user loses
these streams); depends on the maintainers' timing.

**C** — B, consumed before its release through `[patch.crates-io]` on a git fork. A git dependency
in a `--locked` workspace (D-022) and in the web build, for a ~15-line fix.

**D** — A and B: the core workaround now, removed once a lopdf release recovers the stream itself.

Independently: what to do with a stream still lost after recovery — write the output anyway (what
happened so far), or refuse the file.

## 3. What the assistant proposed

**D**, with the workaround following lopdf's own rule for a wrong `/Length` so the two agree, and a
guard: a referenced stream still deferred, empty and without a usable `/Length` makes
`compress_bytes` fail with `UnreadableStream` instead of writing a degraded copy. Also bumping lopdf
to 0.47 in the same change (load and traversal speed-ups, per-filter `/DecodeParms` arrays,
`save_modern` raising the PDF version with an xref stream, a dozen recovery fixes for damaged files).
C was rejected for the reasons above.

## 4. What we decided, and why

**D**, as proposed.

* `stream_recovery.rs` runs right after `Document::load_mem`: every stream with a parser position,
  no content and no `/Length` resolving to a number is read back from the input up to the only
  EOL-framed `endstream` + `endobj`, without crossing the next object's cross-reference offset;
  `set_content` writes the `/Length`. On the form, 32 streams are recovered and `lossless` renders
  pixel-identical to the input.
* The guard runs after `prune_objects`, so an unreferenced broken object does not make us refuse a
  file. It is the D-005 rule ("never worse than the input") applied to what the loader drops
  silently.
* The upstream fix is prepared on a branch of the project's lopdf fork (`hpwxf/lopdf`,
  `fix/recover-missing-stream-length`): in `parser::stream`, when `/Length` is absent altogether and
  loading is lenient, use the same `recover_stream_length`; strict loading, an indirect `/Length` not
  loaded yet and an ambiguous boundary keep today's behaviour. Opening the pull request is left to
  the maintainer of this repository.
* lopdf 0.47: no API change needed. Its 256 MiB default decompression bound (#656) applies to object
  and cross-reference streams at load; `Stream::decompressed_content` stays unbounded, so images are
  unaffected. An indirect `/DecodeParms` is still ignored by lopdf: `image_ops.rs` keeps inlining it.
  On the corpus (399 files, `lossless`), outputs grow by 50 KB in all (+0.01 %, median +5 bytes);
  the largest growth, +22 KB, is a tagged form whose structure tree (`/StructTreeRoot`, 954
  `/StructElem`) 0.45 silently dropped and 0.47 keeps. One timed corpus run: 10.3 s instead of
  12.0 s.

## 5. What would make us change our minds

* A lopdf release that recovers a missing `/Length` on load: delete `stream_recovery.rs`'s recovery
  (keep the guard) and revise this entry.
* `UnreadableStream` reported on files users care about: the next step is the "repairing broken
  PDFs" question of the README, not a looser guard.

## Revision (2026-11-20): `/DecodeParms` left on a stream lopdf compresses; reals written as `f32`

The corpus check of this entry left two files that differ from their input with the font passes
off. Both come from lopdf too, and both existed before:

* **A jsPDF form**: a diagonal seam through a logo. jsPDF writes the logo's soft mask uncompressed
  but with a PNG `/Predictor` in `/DecodeParms` — meaningless without a filter.
  `Document::compress` (`Stream::compress`) adds `/FlateDecode` and keeps the parameters, so
  viewers apply the predictor to raw samples and every row shifts by one byte. Same treatment as
  above: `rust_engine.rs` removes `/DecodeParms` from every stream without `/Filter` before
  `doc.compress()`, and the fork's branch gets the same fix in `Stream::compress`. Corpus: that file
  becomes pixel-identical, one other jsPDF file shrinks by 71 bytes and stays identical, nothing
  else changes.
* **A Chrome (Skia) export**: not a visible defect. lopdf holds reals as `f32` and writes them
  back rounded (`841.91998` → `841.92` in every `/MediaBox`, `602.05078` → `602.0508` in `/W`);
  the 0.00002 pt change of page height alone is enough for poppler to round some text lines to the
  next pixel row (rounding only the `/MediaBox` of the input reproduces the same difference,
  16,342 pixels at 40 dpi). lopdf's `main` keeps reals as `f64` (#674, a breaking change, not
  released yet): taken with the next release, nothing to do in the core.

