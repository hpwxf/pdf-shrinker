# D-009 — Font programs rewritten in-house: subset merging, Type 1 → CFF, CFF subset completion

| | |
|---|---|
| **Identifier** | D-009 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-30 and 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective), revised |
| **Commits** | `7d41753` (`font_merge.rs` CID TrueType, `type1_merge.rs`, `type1_cff.rs`), `32eca47` (at every level), `247e56d` (simple TrueType, `name` table, `cff_subset.rs`), `60564cf` (re-deflate CFF programs), `3801a31` (ordered merge groups, D-012) |

## 1. Context

On the comparison documents, fonts were a large share of what images could not explain. Keynote and
PowerPoint (through Quartz) embed a fresh subset of the same font **on every page** (101 subsets of
Courier New, 1.07 MB, in the slide deck); LaTeX embeds one per included PDF figure (32 Type 1 subsets
of CMR10, 240 KB, in the thesis), eexec-encrypted so Flate barely compresses them; Word embeds
"subsets" that still carry the whole font's tables; Canva and Prince leave CFF subsets with all their
subroutines. iLovePDF merges subsets and converts Type 1 to CFF. Nothing in the Rust ecosystem does
any of this on PDF-embedded programs.

## 2. Options considered

**A** — Leave fonts alone (only Flate). Safe; leaves 2 to 4 MB on the table on the reference documents.

**B** — Merge only byte-identical programs (what dedup already does). Catches almost nothing: every
subset differs.

**C** — Rewrite font programs ourselves, format by format: union of subsets when they provably agree,
conversion of Type 1 to CFF, completion of sloppy CFF subsets — each pass refusing anything it does not
fully understand.

**D** — Re-subset fonts from scratch with a font library (a Rust port of fontTools' subsetter), driven
by the glyphs actually used in content streams. Most general; needs content-stream text decoding and
a font library we do not have.

## 3. What the assistant proposed

**C**, in stages, each one measured and validated outside the repository: fontTools comparison of
every converted glyph's outline and advance (~4,170 glyphs, 0 differences), pixel-identical renders
with poppler (all pages) and Ghostscript (`7d41753`); for `cff_subset.rs`, every kept glyph
byte-identical once desubroutinized and all 1,200+ pages of the 24 affected corpus files
pixel-identical (`247e56d`). D was not recorded as considered.

## 4. What we decided, and why

**C**, at every level including `lossless` (D-007), since text renders pixel-identical:

* **Subset merging** (`font_merge.rs`, `type1_merge.rs`), by union, never by re-subsetting. Subsetters
  keep original glyph identifiers, so subsets of one font can be merged without touching page content
  or width tables: CID TrueType with an `Identity` mapping (per GID), Type 1 (glyphs by name, `Subrs`
  by index, `return`-stub Subrs count as absent, charstrings compared decrypted), simple TrueType
  (per-GID union plus union of `cmap` subtables, result cut to the highest glyph kept). A merge
  **only** happens when the subsets agree: same name, identical fixed parts, identical glyph data
  wherever two define the same glyph, no conflicting encoding.
* **Type 1 → CFF** (`type1_cff.rs`): every Type 1 program becomes `/FontFile3` `/Type1C`, 3–5× smaller;
  outlines are interpreted to absolute coordinates and re-encoded as Type 2, built-in encoding kept
  (TeX fonts' PDF encodings depend on it). **Hint replacement is dropped** (stems merged into one
  sorted, non-overlapping set): the one lossy aspect, affecting only hinted rendering at very small
  sizes. Anything unknown (counter control, multiple-master OtherSubrs, malformed charstrings) leaves
  the font as Type 1.
* **CFF subset completion** (`cff_subset.rs`): unreached subroutines become 1-byte `return` stubs
  without changing the subr bias, CID-keyed fonts lose unused strings and, when `.notdef` is blank,
  every blank glyph through a rebuilt charset. No charstring is rewritten; DICTs are copied verbatim
  except offsets and SIDs. A program that does not shrink is still re-deflated if that helps
  (`60564cf`). `--tune cff_subset=0` turns it off.
* **TrueType `name` table** reduced to English IDs 1–6 in every program we rewrite (Skia writes 7 KB
  of licence text per subset).

Results: slide deck, embedded fonts 2.32 MB → 0.18 MB (same as iLovePDF); thesis, 4.98 MB →
1.18 MB (iLovePDF 0.81–0.94 MB); one corpus file 457 → 20 KB at `high` from
`cff_subset.rs` alone.

## 5. What would make us change our minds

* A rendering difference traced to a merged or converted font on a real document: the agreement
  checks are the safety net, and any hole in them is worth more than the bytes saved. The validations
  above live outside the repository (D-021); a regression would currently only show on the corpus.
* If hint replacement turned out to matter (small-size screen rendering on Windows, where hinting is
  honoured): keep it, or do the conversion only at `extreme*`.
* If the remaining gap (CFF subset merging, TrueType subsets with renumbered GIDs) required a
  `CIDToGIDMap` per font (`docs/compression-strategies.md` §5), that is still C. D would only become
  attractive if we started rewriting content streams anyway.

## Revision (2026-11-20): blank glyphs and subsets that renumber their GIDs

The signal of §5 happened. Recovering the fonts lopdf used to load empty (D-026) made the simple
TrueType merge run on 4-Heights form fonts, and every space of an address field came out as an `X`.
Looking for the cause on the corpus (`lossless`, every page rendered against the input) found the
same hole on files that had nothing to do with that loader bug: a Pages document (5 pages out of 6
damaged) and a Keynote deck (spaces and end-of-paragraph marks drawn as boxes or `(` on ~40
pages), both already wrong before D-026.

Not every subsetter keeps the original GIDs: 4-Heights and Quartz's `AAAAAx+` subsets number their
glyphs in order of use. The agreement check compared glyph data only where *both* subsets have an
outline, so an empty glyph — a space, a zero-width mark — agreed with anything, and the union took
the other subset's outline. A second, separate hole: when a program is rewritten (merged, or trimmed
alone), its `cmap` kept only entries to drawn glyphs or to the codes `0x20`/`0xA0`; Keynote puts
spaces in a font of their own at codes 33 and 34, which then showed `.notdef`.

What changed, in `font_merge.rs`:

* **Simple TrueType**: every `cmap` entry from a code in `FirstChar..=LastChar` to an empty glyph
  is recorded. Another subset may draw that GID only if its own `cmap` sends the same code to it —
  the case of an emptied slot in a subset that keeps original GIDs and the whole `cmap` (Word). Those
  entries are always kept in the rewritten `cmap`.
* **Every TrueType merge** (CID too, which has no `cmap` to compare): an empty glyph agrees with a
  drawn one only if its advance is zero (slot emptied with its metrics) or equal to the drawn
  glyph's. This alone does not separate a space from a glyph at a fixed pitch.
* A bare subset prefix (`AAAAAA+`, no name after it) is no font name: such programs are no longer
  grouped together (`font_merge.rs` and `type1_merge.rs`).

Effect on the corpus (399 files, `lossless`, every page rendered at 40 dpi against the input): 1,258
merges → 1,254, the four refused being the four wrong ones; pixel-identical files 380 → 383 (the
Pages document, the Keynote deck, and a third file whose only defect was the trimmed `cmap`). The
two files that still differ do so identically before and after, with the font passes turned off
too: two lopdf behaviours, see D-026's revision.

Still open: CID TrueType subsets that renumber their GIDs, at a fixed pitch, with a space at a GID
another subset draws, would still merge wrongly — no `cmap` to tell, and the `/W` widths agree. None
in the corpus.

