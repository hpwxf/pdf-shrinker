# D-010 — Image kinds: CMYK stays CMYK, pure-Rust codecs, bi-level at 2×, palettes and soft masks lossless

| | |
|---|---|
| **Identifier** | D-010 |
| **Date** | 2026-11-20 (retrospective; decisions taken from 2026-09-28 to 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (gray/RGB JPEG and Flate, SMask resized and kept Flate), `7d41753` (palettes, near-gray, opaque SMask, PNG-predicted SMask, `[/FlateDecode /DCTDecode]`), `fe6a6d2` (CMYK, JPEG 2000, Indexed, 16-bit, 1-bit → G4), `31ea3cd` (bi-level downsampling), `60564cf` (`/Matte`), `a896ce3` (palette at own resolution, black-and-white gray → G4) |

## 1. Context

The first engine handled 8-bit gray and RGB images stored as JPEG or Flate, and skipped everything
else. Real documents carry CMYK (print), JPEG 2000 (scanners, some exporters), palettes (diagrams),
16-bit samples, 1-bit scans and stencil masks, soft masks, and images already processed by iLovePDF
(`[/FlateDecode /DCTDecode]`). Each kind raises the same two questions: can we decode it without a C
dependency (D-003), and what is the right output encoding without changing how the page looks?

## 2. Options considered

For colour, **A** convert everything to RGB JPEG (simplest, smallest), or **B** keep each image in its
colour space, CMYK included.

For few-colour images, **A** JPEG like any other, or **B** a lossless `Indexed` palette when it is not
much bigger.

For bi-level images, **A** keep them as they are, **B** re-encode losslessly as CCITT G4, or **C**
also downsample them, at a resolution suited to bi-level content.

For soft masks, **A** JPEG like their parent, or **B** always lossless.

## 3. What the assistant proposed

**B** throughout, with **C** for bi-level, each argued in the commits: CMYK "re-encoded as a CMYK JPEG,
never converted to RGB" (`fe6a6d2`); bi-level "needs more pixels to stay legible" (`31ea3cd`); SMasks
"never re-encoded to JPEG, to avoid alpha artifacts" (`CLAUDE.md`).

## 4. What we decided, and why

* **CMYK stays CMYK** (device or `ICCBased` N=4): re-encoded as a CMYK JPEG with the usual
  downsampling and quality logic, so print workflows stay valid and no colour-management dependency
  is needed. An Adobe-marker CMYK JPEG source keeps its stored samples and `/Decode`; any other CMYK
  source is written inverted with `/Decode [1 0 1 0 1 0 1 0]` (Photoshop/img2pdf convention). The
  colour space object is kept.
* **Pure-Rust decoders**: JPEG 2000 with `hayro-jpeg2000` (default features off), CCITT G4 with
  `fax`. A JPX alpha channel with `/SMaskInData` becomes a real `/SMask`. 16-bit samples are reduced to
  8 bits. `Indexed` images are decoded through their lookup table.
* **Lossless palette** when an image has ≤ 256 colours and the `Indexed` + PNG predictor + Flate result
  is at most 12.5 % larger than the JPEG: no ringing around text and flat areas. When the image would
  be downsampled, the palette at its *own* resolution is tried too, since resampling adds edge colours
  (`a896ce3`: a 23-colour map, 124 KB as a full-size palette vs 169 KB as a 200 dpi JPEG).
* **Near-gray RGB → gray** when channels never differ by more than 2 levels.
* **Bi-level** (1-bit raw/Flate/G4, stencil masks, and 8-bit gray holding only black and white):
  downsampled to the level's colour targets × `BILEVEL_RESOLUTION_FACTOR` (2) — 300 dpi at `medium`,
  192 at `high`/`extreme`, 144 at `extreme-max` — averaging ink coverage and thresholding so as to keep
  the source's ink proportion, never above 35 % coverage (thin strokes get slightly bolder instead of
  vanishing); then CCITT G4 if smaller. Lossless when not downsampled.
* **Soft masks always lossless**, resized with their parent, dropped when fully opaque, PNG-predicted
  when smaller. A mask with `/Matte` **pins its parent's colour space** (no gray, no palette; a palette
  parent is not re-encoded at all), because the matte is expressed in that space and the mask may be
  shared (`60564cf`).
* **Left untouched**, counted as skipped: JBIG2, CCITT G3, Lab, Separation/DeviceN, JPX with an explicit
  palette.

## 5. What would make us change our minds

* CMYK → RGB is on `TODO.md` for documents only meant for the screen (no output intent, no spot
  colours), at `extreme*`. It would be the first pass that changes colours, and needs either `lcms2`
  (C, against D-003) or a naive formula whose error must be measured first.
* If JBIG2 encoding became available in Rust (or worth writing), it would beat G4 on bi-level text,
  as iLovePDF's "extreme" does — but its 72 dpi output is barely legible, so the resolution rule
  above stays.
* Lossy soft masks (shadows, gradients) could save 10–20 % on masks; only if a halo-free method is
  shown on real documents.
