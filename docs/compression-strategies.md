# Compression strategies by level

This document describes what `pdfshrink-core` does at each compression level, then compares the
result with iLovePDF on three real documents of very different kinds: a slide deck exported from
Keynote, a LaTeX thesis, and a scanned exam. The `extreme-safe`, `extreme` and `extreme-max`
levels are **experimental**: they are available from the CLI only (`-l extreme`, …), not in the
app.

## 1. Common pipeline

Every level goes through the same built-in Rust engine (`rust_engine.rs`; there is no external
tool), in this order. Steps in
*italics* only run at the `extreme*` levels.

1. **Cleanup**: drop unreachable objects, merge streams that are byte-for-byte identical (same
   dictionary, same content).
2. *Deep deduplication* (`deep_dedup.rs`): merge objects that are identical **once decoded**,
   repeated until nothing changes (see §3.1).
3. *Font subset merging* (`font_merge.rs` for TrueType, `type1_merge.rs` for Type 1), then
   *Type 1 → CFF conversion* (`type1_cff.rs`), §3.3.
4. **Images** (`image_ops.rs`): each eligible image is decoded → optionally cropped → optionally
   cleaned up if it is a scanned page → optionally downsampled → re-encoded, in parallel. An image
   is only replaced if the result is smaller.
5. *Second deduplication pass*: two images that only differed by their encoding now produce the
   same bytes.
6. **Flate** on every uncompressed stream; *then Zopfli* on non-image streams (§3.4).
7. **Write** as a modern PDF 1.5 (object streams + compressed xref table), reload, and check that
   the page count is unchanged. The file is only kept if it is smaller than the original.

Eligible images (all lossy levels): gray, RGB and CMYK (device or `ICCBased`), palette
(`Indexed`) and 16-bit images, stored as JPEG (`DCTDecode`, also when wrapped in an extra Flate
layer as `[/FlateDecode /DCTDecode]`, which is how iLovePDF stores them), JPEG 2000 (`JPXDecode`)
or raw samples (uncompressed or `FlateDecode`, with or without a PNG predictor); plus 1-bit images
and stencil masks stored raw, in Flate or as CCITT Group 4, which take a bi-level path (§3.7).
Left untouched: JBIG2 and CCITT Group 3 images, Lab, separation/DeviceN and other exotic colour
spaces.

## 2. Settings by level

| | lossless | low | medium | high | extreme-safe | extreme | extreme-max |
|---|---|---|---|---|---|---|---|
| Image re-encoding | no | JPEG | JPEG | JPEG | JPEG / palette | JPEG / palette | JPEG / palette |
| JPEG quality | – | 85 fixed | 75 fixed | 55 fixed | 40–70, SSIM ≥ 0.985 | 35–65, SSIM ≥ 0.975 | 30–55, SSIM ≥ 0.96 |
| JPEG quality, scanned pages | – | 85 | 75 | 55 | 55 fixed | 50 fixed | 42 fixed |
| Target DPI (trigger) | – | 300 (> 450) | 150 (> 225) | 96 (> 144) | 96 (> 144) | 96 (> 115) | 72 (> 79) |
| Longest-side cap | – | 4200 px | 3000 px | 2000 px | 2000 px | 1800 px | 1400 px |
| Page-relative cap | – | – | – | – | – | 2200 px wide | 1600 px wide |
| Byte-level dedup | yes | yes | yes | yes | yes | yes | yes |
| Deep dedup | – | – | – | – | yes | yes | yes |
| Font merging (TrueType, Type 1) | – | – | – | – | yes | yes | yes |
| Type 1 → CFF conversion | – | – | – | – | yes | yes | yes |
| Zopfli | – | – | – | – | yes | yes | yes |
| Near-gray RGB → gray | – | – | – | – | yes | yes | yes |
| Lossless palette (≤ 256 colours) | – | – | – | – | yes | yes | yes |
| Opaque SMask dropped, SMask PNG-predicted | – | – | – | – | yes | yes | yes |
| Crop to visible area | – | – | – | – | no | yes | yes |
| Scanned paper whitened | – | – | – | – | no | yes | yes |

How to read it: an image is only **downsampled** if its effective resolution exceeds the
threshold in parentheses, or if it exceeds one of the caps. It is, however, **always re-encoded**
(a raw or Flate image gains a lot from the switch to JPEG alone), at unchanged dimensions if no
threshold is crossed.

`extreme-safe` never changes what a page looks like beyond compression artifacts: no cropping, no
paper whitening. `extreme` and `extreme-max` allow both.

Every setting of the experimental levels can be overridden on the fly to try variants:
`--tune key=value` (keys: `dpi`, `trigger`, `quality`, `max_dim`, `page_px`, `ssim`,
`min_quality`, `dedup`, `fonts`, `cff`, `zopfli`, `gray`, `palette`, `opaque_smask`, `crop`,
`scan_whiten`).

## 3. The strategies in detail

### 3.1 Repeated images (deduplication)

Slide exports often store a fresh copy of the same image on every page it appears on (logo,
background, repeated screenshot). Two levels of detection:

- **Byte-level** (all levels): only merges streams that are strictly identical, dictionary
  included.
- **After decoding** (`extreme*`): compares the *decoded* content and the dictionary minus its
  encoding keys (`Filter`, `DecodeParms`, `Length`). This catches copies encoded differently and,
  by iterating, images whose only difference was the pointer to their transparency mask
  (`/SMask`): once the masks are merged, the images become identical in turn. Dictionaries with no
  identity of their own (fonts, font descriptors, graphics states, colour spaces…) are merged the
  same way; pages, annotations and structure elements never are.

### 3.2 Over-resolved images

- **Effective resolution** (`placement.rs`): each page's content is walked while tracking the
  current transformation matrix, which gives the actual size each image is drawn at. When an
  image is drawn several times, the most demanding (largest) placement wins.
- **Page-relative cap** (`extreme`, `extreme-max`): DPI is meaningless for a slide exported at
  1920×1080 **points**, where 96 dpi already means 2560 px across. The cap is therefore expressed
  in pixels across the page's **displayed width** (its height if the page is rotated by 90°). The
  longest side would be the wrong choice: a web page exported at 1512×14400 pt is read at its
  width.
- **Absolute cap**: a safety net on the longest side, independent of placement.

### 3.3 Fonts

Exports embed a **fresh subset of the same font over and over**: Keynote/PowerPoint (through
Quartz) once per page, LaTeX once per included PDF figure. Subsetters keep the original glyph
identifiers and only drop what isn't used, so the subsets of one font can be merged by union. Two
formats are handled (`extreme*`):

- **TrueType** (`CIDFontType2` with an `Identity` mapping, typical of slide exports), in
  `font_merge.rs`: union glyph by glyph number, touching neither page content nor width tables.
- **Type 1** (`/FontFile`, typical of LaTeX and its Computer Modern fonts), in `type1_merge.rs`.
  These programs are `eexec`-encrypted, so Flate barely compresses them and every copy costs its
  full size. The merge decrypts them, takes the union of glyphs by name and of subroutines
  (`Subrs`) by index, and re-encrypts. Subsetters replace unused subroutines with a bare `return`
  stub; those count as absent. Each glyph is encrypted with random leading bytes, so glyphs are
  compared once decrypted.

A merge only happens when the subsets agree: same font name, identical fixed parts (header,
hinting programs or `/Private` dictionary), identical glyph data wherever two subsets define the
same glyph, and no conflicting built-in encoding. Text renders **pixel-identical**: on the LaTeX
thesis below, all 213 pages rendered at 72 dpi are the same with and without merging.

**Type 1 → CFF conversion** (`extreme*`, `type1_cff.rs`). After merging, every Type 1 program is
rewritten as a CFF program (`/FontFile3` with `/Subtype /Type1C`): the same outlines, stored as
plain, compact Type 2 charstrings instead of doubly encrypted Type 1 ones, typically 3 to 5 times
smaller. PDF viewers accept a CFF program for a `/Type1` font dictionary, so only the font
descriptor changes. Each glyph is interpreted into an absolute outline (subroutines expanded,
*flex* turned into its two curves, accented characters kept as a Type 2 `seac`), then re-encoded
with compact operators (`hlineto`/`vlineto` chains, `hvcurveto`/`vhcurveto`, batched curves). The
font's built-in encoding is preserved, since TeX fonts' PDF encodings are expressed relative to
it. Hints are simplified: all the stems of a glyph are kept as one sorted, non-overlapping set, and
hint replacement is dropped, which only matters for hinted rendering at small sizes. A font using
anything the converter doesn't understand (counter-control or multiple-master OtherSubrs, a
malformed charstring…) stays Type 1.

Validation, done outside the repository: on 5 documents (125 fonts, ~4,170 glyphs), fontTools
finds every CFF glyph's outline and advance width identical to the Type 1 original, and every page
renders pixel-identical before and after conversion, with poppler (all pages) and Ghostscript
(40 pages of the thesis).

Not handled: merging CFF subsets (`FontFile3`, e.g. the `f-0-0` fonts cairo writes for
matplotlib figures), simple TrueType fonts (`/TrueType`, whose `cmap` differs from one subset to
the next), and Type3 fonts (only merged when strictly identical).

### 3.4 Image and stream encoding

- **Adaptive JPEG quality** (`extreme*`): binary search for the lowest quality whose SSIM, computed
  against the (already resized) source image, stays above the level's threshold. The score is the
  **luma** SSIM, capped by the worst RGB channel's SSIM + 0.03: luma drives it, while damage to a
  thin coloured line (chroma subsampling) still vetoes a quality. Consequence: a fine texture may
  get a *higher* quality than in `high`.
- **Lossless palette** (`extreme*`): an image with ≤ 256 colours (icon, logo, diagram) is stored as
  `Indexed` at 1/2/4/8 bits + PNG predictor + Flate, provided the result is no more than 12.5 %
  larger than the JPEG. This avoids JPEG ringing around text and flat areas.
- **Gray**: an RGB image whose three channels never differ by more than 2 levels is stored as gray
  (no chroma to encode).
- **Transparency masks (SMask)**: always lossless (JPEG on an alpha channel produces visible halos).
  Resized along with their image; at `extreme*`, dropped when fully opaque, and stored with a PNG
  predictor when that is smaller.
- **Zopfli** (`extreme*`): stronger Flate recompression of non-image streams (page content, fonts,
  forms), still readable by any viewer. Typical gain 5–8 % on those streams, at a high CPU cost
  (hence ~45 s for `extreme` versus ~5 s for `high` on the slide deck).

### 3.5 Under-used images (cropping)

A full-frame 3:2 photo placed on a 16:9 slide overflows the page: about 15 % of its pixels are
never visible. At `extreme` and `extreme-max`, the image is cropped to its visible part (the union
over all its placements, intersected with the MediaBox, plus a 2 px margin). The cropped image is
drawn by a small Form XObject that takes over the original image's object number: no page content
is rewritten.

Safeguards: only if *every* reference to the image was seen by the page walk, that walk is
complete, and the image has no `/Mask`, no optional content (`/OC`), no structure tagging and no
shared transparency mask. Disabled at `extreme-safe`: poppler smooths an image drawn through a
Form slightly differently when zoomed far out.

### 3.6 Scanned pages

A page is treated as a scan when a single image spans at least 80 % of the page width and at least
60 % of its pixels are light, low-saturation "paper" whose median tone is below 254. That last
condition keeps screenshots and digitally produced pages out: their background is exactly 255.

- **Fixed JPEG quality** (all `extreme*` levels): the middle of the level's quality range, instead
  of the SSIM search. SSIM rewards reproducing scanner noise and earlier JPEG artifacts, which
  pushed scans to the highest quality for no visible benefit.
- **Paper whitening** (`extreme`, `extreme-max`): a linear levels stretch that maps the paper tone
  (its median minus 20) to white. JPEG then stops spending bits on paper grain and shading: about
  25 % smaller at equal quality on a 150 dpi scan. Linear rather than a threshold, so faint pencil
  strokes get lighter but never vanish. Colours are kept (headings, red corrections).

### 3.7 Other image kinds

- **CMYK** (print PDFs): re-encoded **as a CMYK JPEG**, with the usual downsampling and quality
  logic; colours are not converted to RGB, so print workflows stay valid. A CMYK JPEG source with
  an Adobe marker keeps its stored samples and its `/Decode` array, so it renders exactly as the
  original did in every viewer. Any other CMYK source (raw, Flate, JPEG 2000) is written the way
  Photoshop and img2pdf do: samples inverted, `/Decode [1 0 1 0 1 0 1 0]`. The colour space
  (`DeviceCMYK` or its ICC profile) is kept. The SSIM search uses the mean of the four channels,
  with the worst channel as a guard.
- **JPEG 2000**: decoded with a pure-Rust decoder (`hayro-jpeg2000`), then re-encoded as JPEG (or
  palette) with the usual logic. An alpha channel inside the codestream (`/SMaskInData`) becomes a
  real `/SMask`.
- **Palette images** (`Indexed`): decoded through their lookup table, then the normal path; a
  lossless palette re-encoding is always tried, since they can't have more than 256 colours
  unless resized.
- **16-bit images**: reduced to 8 bits, then the normal path.
- **1-bit images and stencil masks** (raw, Flate or CCITT Group 4): downsampled when
  over-resolved, then encoded as **CCITT Group 4** (pure-Rust `fax` codec) when that's smaller.
  Bi-level images need about twice the resolution of gray/colour ones to stay legible, so their
  targets are the level's colour targets × 2: 300 dpi at `medium` (a 300 dpi scan is left at full
  resolution), 192 dpi at `high`/`extreme`, 144 dpi at `extreme-max`. Downsampling averages ink
  coverage over each new pixel, then thresholds it: the threshold keeps the source's proportion of
  ink, but never goes above 35 % coverage, so a stroke thinner than the new pixel is kept (slightly
  bolder) rather than dropped. Without downsampling, the re-encoding is lossless. On a real 300 dpi
  black-and-white text page (21.7 KB as libtiff's G4): 20.3 KB at `medium` (lossless), 13.6 KB at
  192 dpi, 10.3 KB at 144 dpi, all legible. For comparison, iLovePDF goes to 150 dpi CCITT
  ("recommended") and 72 dpi JBIG2 ("extreme"), the latter barely legible for text.

Test PDFs for all of these: `scripts/make-test-pdfs.sh` (see `TODO.md`).

## 4. Comparison with iLovePDF

iLovePDF's strategies are not public: what follows is **inferred from its output files**. Figures
produced with `cargo run --release -p pdfshrink-core --example analyze`. SSIM: structural
similarity between each page's rendering and the reference's (poppler, 40 dpi, grayscale),
averaged over pages; 1 = identical. Times measured on an Apple Silicon Mac. iLovePDF's modes are
named as in its English interface: "recommended" and "extreme compression".

### 4.1 Slide deck: `rust-1.pdf`

132 slides at 1920×1080 pt exported from Keynote, 103 MB: lots of repeated images and per-page
font subsets.

**Overview**

| | Size | Mean SSIM | Worst-page SSIM | Time |
|---|---|---|---|---|
| Original | 103.4 MB | 1 | 1 | – |
| iLovePDF recommended | 13.5 MB | 0.9887 | 0.860 | – |
| iLovePDF extreme | 11.1 MB | 0.9862 | 0.860 | – |
| pdfshrink `high` | 13.6 MB | 0.9960 | 0.933 | 5 s |
| pdfshrink `extreme-safe` | 9.1 MB | 0.9965 | 0.949 | 43 s |
| pdfshrink `extreme` | 7.6 MB | 0.9947 | 0.911 | 47 s |
| pdfshrink `extreme-max` | 6.2 MB | 0.9911 | 0.863 | 51 s |

**Where the bytes go**

| Category | Original | iLovePDF rec. | iLovePDF extreme | `high` | `extreme-safe` | `extreme` | `extreme-max` |
|---|---|---|---|---|---|---|---|
| Images | 79.03 MB | 8.76 | 6.58 | 6.70 | 4.87 | 3.49 | 2.21 |
| Transparency masks | 11.10 MB | 0.81 | 0.69 | 1.44 | 1.06 | 0.87 | 0.68 |
| Embedded fonts | 2.32 MB | 0.18 | 0.18 | 2.25 | 0.18 | 0.18 | 0.18 |
| Page content | 1.50 MB | 1.33 | 1.25 | 1.50 | 1.36 | 1.36 | 1.36 |
| Other streams (Type3, patterns, ICC…) | 7.41 MB | 0.59 | 0.58 | 0.69 | 0.65 | 0.65 | 0.65 |
| Form XObjects | 0.33 MB | 0.28 | 0.26 | 0.33 | 0.31 | 0.31 | 0.31 |

**Images**

| | Original | iLovePDF rec. | iLovePDF extreme | `high` | `extreme-safe` | `extreme` | `extreme-max` |
|---|---|---|---|---|---|---|---|
| Image objects (masks included) | 3815 | 638 | 638 | 2204 | 638 | 638 | 638 |
| Remaining duplicates (after decoding) | 3180 | 4 | 4 | 1577 | 17 | 27 | 16 |
| Images (masks excluded) | 2105 | 416 | 416 | 1980 | 414 | 414 | 414 |
| of which longest side < 256 px | 400 | 218 | 235 | 588 | 227 | 230 | 243 |
| 256–1023 px | 702 | 85 | 89 | 528 | 87 | 93 | 88 |
| 1024–2000 px | 955 | 81 | 65 | 864 | 100 | 91 | 83 |
| > 2000 px | 48 | 32 | 27 | 0 | 0 | 0 | 0 |
| Largest image | 6673 px | 6673 px | 6673 px | 2000 px | 2000 px | 1800 px | 1400 px |
| Total pixels | 770 Mpx | 200 Mpx | 152 Mpx | 566 Mpx | 100 Mpx | 83 Mpx | 62 Mpx |
| Image encoding | 2057 Flate, 48 JPEG | 187 JPEG¹, 227 Flate | 198 JPEG¹, 216 Flate | 832 JPEG, 1148 Flate | 224 JPEG, 190 Flate² | 221 JPEG, 193 Flate² | 222 JPEG, 192 Flate² |
| Colour spaces | 1633 RGB, 445 ICC, 27 gray | 283 RGB, 96 ICC, 35 gray | 370 RGB, 9 ICC, 35 gray | 1750 RGB, 204 ICC, 26 gray | 198 RGB, 186 palette, 30 gray | 196 RGB, 190 palette, 27 gray | 196 RGB, 189 palette, 28 gray |
| Mask encoding | 1710 Flate | 118 Flate, 104 JPEG | 106 Flate, 116 JPEG | 224 Flate | 224 Flate | 224 Flate | 224 Flate |

¹ iLovePDF wraps most of its JPEGs in an extra Flate filter (`[/FlateDecode /DCTDecode]`).
² In pdfshrink, the remaining Flate images are the lossless palettes and small images that JPEG
would not have made smaller.

The duplicates left by pdfshrink are tiny images (0 to 0.04 MB reclaimable), usually identical
pixels with slightly different dictionaries.

**The three heaviest images in each version**

| Version | 1st | 2nd | 3rd |
|---|---|---|---|
| Original | 1280×1000, Flate, 3.19 MB | 4000×1552, Flate, 2.19 MB | 3496×1420, Flate, 2.15 MB |
| iLovePDF rec. | 2560×1440, 697 KB | 4000×2251, 561 KB | 2016×1365, 452 KB |
| iLovePDF extreme | 2560×1440, 697 KB | 2400×1351, 434 KB | 1920×1080, 208 KB |
| `high` | 2000×1328, 399 KB | 2000×1328, 399 KB (duplicate) | 2000×1354, 310 KB |
| `extreme-safe` | 2000×1328, 525 KB³ | 2000×1354, 397 KB | 2000×1330, 381 KB |
| `extreme` | 1800×1015 (cropped), 342 KB | 1800×1016 (cropped), 255 KB | 1371×928, 200 KB |
| `extreme-max` | 1400×790 (cropped), 183 KB | 1400×790 (cropped), 151 KB | 997×675, 100 KB |

³ Heavier than in `high`: it is a very fine rust texture, for which `extreme-safe`'s SSIM
threshold requires a JPEG quality above 55.

**Fonts and other embedded elements**

| | Original | iLovePDF (both) | `high` | `extreme*` |
|---|---|---|---|---|
| Embedded font programs | 256 (2.32 MB) | 22 (0.18 MB) | 226 (2.25 MB) | 22 (0.18 MB) |
| Type3 font dictionaries | 870 | 654 | 870 | 654 |
| CIDFontType2 dictionaries | 256 | 230 | 256 | 194 |
| ICC profiles | 149 | 4 (rec.) / 1 (extreme) | 4 | 0–1 |
| Total object count | 52,866 | 11,543 / 7,617 | 10,620 | ~7,580 |

Font merging example: the 101 subsets of Courier New (1.07 MB in total) become a single 27 KB
program, in iLovePDF as in pdfshrink.

**Takeaways.** Both tools deduplicate on decoded content (same final image count, 638) and merge
font subsets (22 programs each). The size gap comes mostly from images: iLovePDF keeps 27 to 32
images larger than 2000 px, up to 6673 px, i.e. 2 to 3 times more pixels than `extreme`, while
pdfshrink caps every image, adapts JPEG quality per image, keeps ~190 small images as lossless
palettes and crops photos that overflow the page. pdfshrink is smaller *and* closer to the
original at every experimental level, including on the worst page (0.911 at `extreme` vs 0.860).

### 4.2 LaTeX thesis: `18635-HDR_Francesco_Sanfedino_vfinal.pdf`

213 A4 pages from pdfLaTeX (via HAL), 34.3 MB: figures included as PDF or PNG, each carrying its
own subsets of the Computer Modern fonts, plus vector graphics.

**Overview**

| | Size | Mean SSIM | Worst-page SSIM | Time |
|---|---|---|---|---|
| Original | 34.3 MB | 1 | 1 | – |
| iLovePDF recommended | 6.6 MB | 0.9985 | 0.974 | – |
| iLovePDF extreme | 5.5 MB | 0.9961 | 0.934 | – |
| pdfshrink `high` | 9.0 MB | 0.9981 | 0.968 | 1 s |
| pdfshrink `extreme-safe` | 5.9 MB | 0.9981 | 0.968 | 27 s |
| pdfshrink `extreme` | 5.8 MB | 0.9981 | 0.968 | 26 s |
| pdfshrink `extreme-max` | 5.6 MB | 0.9970 | 0.943 | 27 s |

**Where the bytes go**

| Category | Original | iLovePDF rec. | iLovePDF extreme | `high` | `extreme-safe` | `extreme` | `extreme-max` |
|---|---|---|---|---|---|---|---|
| Embedded fonts | 4.98 MB | 0.94 | 0.81 | 4.12 | 1.18 | 1.18 | 1.18 |
| Form XObjects (vector figures) | 3.13 MB | 2.58 | 2.58 | 3.13 | 2.81 | 2.81 | 2.81 |
| Images | 24.33 MB | 1.46 | 0.55 | 0.75 | 0.82 | 0.77 | 0.50 |
| Transparency masks | 0.79 MB | 0.06 | 0.03 | 0.05 | 0.05 | 0.05 | 0.03 |
| Page content | 0.63 MB | 0.62 | 0.62 | 0.63 | 0.60 | 0.60 | 0.60 |

**Images**

| | Original | iLovePDF rec. | iLovePDF extreme | `high` | `extreme-safe` | `extreme` | `extreme-max` |
|---|---|---|---|---|---|---|---|
| Images (masks excluded) | 164 | 159 | 159 | 163 | 158 | 158 | 158 |
| > 2000 px | 25 | 0 | 0 | 0 | 0 | 0 | 0 |
| Largest image | 7257 px | 1849 px | 1849 px | 1849 px | 1849 px | 1800 px | 1400 px |
| Total pixels | 261 Mpx | 15.9 Mpx | 4.5 Mpx | 8.0 Mpx | 7.3 Mpx | 7.3 Mpx | 4.3 Mpx |
| Encoding | 138 Flate, 26 JPEG | 125 JPEG¹, 34 Flate | 148 JPEG¹, 11 Flate | 151 JPEG, 12 Flate | 115 JPEG, 43 Flate/palette | 120 JPEG, 38 Flate/palette | 111 JPEG, 47 Flate/palette |

¹ Wrapped in Flate, as in §4.1.

**Fonts**

| | Original | iLovePDF rec. | iLovePDF extreme | `high` | `extreme*` |
|---|---|---|---|---|---|
| Embedded font programs | 823 | 436 | 421 | 712 | 438 |
| Size | 4.98 MB | 0.94 MB | 0.81 MB | 4.12 MB | 1.18 MB (1.97 MB merged but still Type 1) |
| Type 1 programs | 423 | 0 | 0 | 348 | 0 (merged to 76, then converted) |
| Example: CMR10 | 32 Type 1 subsets, 240 KB | 16 CFF programs, 23 KB | – | 25 Type 1 subsets, 240 KB | 1 CFF program |

**Takeaways.** The fonts decide this document. Both tools merge subsets to about the same number
of programs (436 vs 438) and **convert Type 1 fonts to CFF** (iLovePDF renames them `f-0-0`,
`f-1-0`…): pdfshrink's fonts drop from 4.12 MB (`high`) to 1.97 MB with merging alone, then to
1.18 MB with CFF conversion. iLovePDF still ends up slightly lower (0.81–0.94 MB), mostly on the
simple TrueType fonts (e.g. 13 Times New Roman subsets) that neither tool merges but iLovePDF
seems to trim further. Images are on par (0.50 MB at `extreme-max` vs 0.55 MB for iLovePDF
extreme), vector figures slightly smaller on iLovePDF's side (2.58 vs 2.81 MB). Overall,
`extreme-safe` is clearly smaller than iLovePDF recommended at equal quality (5.9 vs 6.6 MB), and
`extreme-max` matches iLovePDF extreme (5.6 vs 5.5 MB) with a slightly better SSIM.
Also worth noting: before a bug fix made along the way (images whose `/DecodeParms` is an
indirect object were silently skipped), `high` produced 13.1 MB on this file; it now gives 9.0 MB.

### 4.3 Scanned exam: `CC1_4AL1`

46 A4 pages of students' exam papers, scanned at 150 dpi in colour: printed questions with
coloured headings, handwritten answers. The only available input is iLovePDF's "recommended"
output (8.8 MB), so it is both the starting point and the SSIM reference here; iLovePDF's extreme
version was produced from it too.

**Overview**

| | Size | Resolution | Mean SSIM | Worst-page SSIM | Time |
|---|---|---|---|---|---|
| Input (iLovePDF recommended) | 8.8 MB | 150 dpi, 1240×1750 | 1 | 1 | – |
| iLovePDF extreme | 2.8 MB | 72 dpi, 595×842 | 0.899 | 0.882 | – |
| pdfshrink `high` | 2.8 MB | 96 dpi, 794×1122 | 0.963 | 0.958 | < 1 s |
| pdfshrink `extreme-safe` | 2.8 MB | 96 dpi | 0.963 | 0.958 | 1 s |
| pdfshrink `extreme` | 2.3 MB | 96 dpi, paper whitened | 0.933⁴ | 0.919⁴ | 1 s |
| pdfshrink `extreme-max` | 1.3 MB | 72 dpi, paper whitened | 0.913⁴ | 0.899⁴ | 1 s |

⁴ Understated on purpose: whitening the paper changes every background pixel, which SSIM counts
as a difference even though legibility improves.

**Structure**

| | Input | iLovePDF extreme | `high` / `extreme-safe` | `extreme` | `extreme-max` |
|---|---|---|---|---|---|
| Images | 46 | 46 | 46 | 46 | 46 |
| Encoding | JPEG in Flate, ICC colour | JPEG in Flate, RGB | JPEG, RGB | JPEG, RGB | JPEG, RGB |
| Total pixels | 99.5 Mpx | 22.9 Mpx | 40.8 Mpx | 40.8 Mpx | 22.9 Mpx |
| Image bytes | 9.23 MB | 2.88 MB | 2.94 / 2.93 MB | 2.42 MB | 1.34 MB |

**Takeaways.** iLovePDF's extreme mode simply halves the resolution again (150 → 72 dpi), which
makes handwriting visibly blurrier. At the same size, pdfshrink `high` keeps 96 dpi. `extreme`
whitens the paper and uses a fixed quality suited to scans, which gives a smaller file than
iLovePDF extreme at a higher resolution. `extreme-max` goes down to the same 72 dpi as iLovePDF
extreme, but at under half its size. On a scan, the image *is* the page: there are no fonts or
duplicates to exploit, so the gains come only from resolution, quality and background cleanup.

Supporting this file required reading iLovePDF's `[/FlateDecode /DCTDecode]` images, which
pdfshrink previously skipped entirely (0 % gain). That now also benefits any PDF that has been
through iLovePDF before.

### 4.4 Why there is no Ghostscript engine any more

pdfshrink used to offer an optional engine that shelled out to a Homebrew-installed Ghostscript
(`-dPDFSETTINGS=/screen`, images at 96 dpi), plus a "best of both" mode keeping the smaller
output. It was removed after this comparison (SSIM as above, worst page in parentheses):

| Document | Rust `high` | Ghostscript `high` | Rust `extreme` |
|---|---|---|---|
| Slide deck rust-1 (103 MB) | 13.6 MB · 0.996 | 16.0 MB · 0.987 (0.896), 73 s | 7.6 MB · 0.995 |
| LaTeX thesis (34 MB) | 9.0 MB · 0.998 | 5.7 MB · 0.990 (0.797) | 5.8 MB · 0.998 |
| Scanned exam (8.8 MB) | 2.8 MB · 0.963 | 4.6 MB · 0.912 | 2.3 MB · 0.933 |
| Business proposal (6.5 MB) | 1.4 MB · 0.998 | 0.54 MB · 0.966 (0.870) | 0.40 MB · 0.997 |
| O'Reilly book (2.7 MB) | 2.1 MB · 1.000 | 2.7 MB · 0.999 | 1.9 MB · 1.000 |
| Manual (2.2 MB) | 2.1 MB · 0.990 | 2.1 MB · 0.985 | 2.0 MB · 0.994 |
| Web page export (5.5 MB) | 0.72 MB · 0.999 | 0.89 MB · 0.996 | 0.56 MB · 0.998 |

Ghostscript was never both smaller and closer to the original than `extreme`. When it beat
`high` on size, it was at a visible quality cost, which the "best of both" mode (picking the
smaller file) would silently accept. It also rotated some pages (its default `AutoRotatePages`
added a `/Rotate` to one slide and two thesis pages), warned about transparency colour spaces,
and required an external AGPL binary. What it could still do that pdfshrink can't — re-encode
CMYK, JPEG 2000, JBIG2 or CCITT images, repair broken files — is on the to-do list for the Rust
engine instead (see `TODO.md`).

## 5. Known limitations and next steps

- CFF (`FontFile3`) and simple TrueType subsets are not merged; Type3 fonts only when strictly
  identical. That's the remaining font gap with iLovePDF on the thesis (1.18 vs 0.81–0.94 MB).
- Type 1 → CFF conversion keeps outlines exact but simplifies hints (no hint replacement), which
  could slightly change hinted rendering at very small sizes.
- JBIG2 (generic region) encoding would compress bi-level images better than CCITT G4; JBIG2 and
  CCITT Group 3 inputs are left as they are. For colour scans, a bi-level mode for pages without
  colour, or a mixed raster content split (sharp text mask + low-resolution colour background),
  would go much further.
- CMYK images are never converted to RGB, even for documents only meant for the screen.
- No rewriting of page content or vector figures (number rounding, removal of useless operators),
  which iLovePDF seems to do lightly.
- Transparency masks are always lossless: a JPEG mode for "soft" masks (shadows, gradients) could
  save another 10 to 20 % on masks.
- SSIM thresholds, `page_px` and scan detection are calibrated on only a handful of documents.

## 6. Reproducing the measurements

```bash
cargo build --release -p pdfshrink-cli -p pdfshrink-core --examples
./target/release/pdfshrink -l extreme --suffix extreme rust-1.pdf
./target/release/examples/analyze rust-1-extreme.pdf        # byte breakdown, images, fonts
./target/release/examples/inspect_images rust-1-extreme.pdf # every image listed
./target/release/examples/fonts rust-1-extreme.pdf          # embedded font programs per font
PDFSHRINK_DEBUG=1 ./target/release/pdfshrink -l extreme rust-1.pdf  # per-phase timings, JPEG qualities
```

SSIM was computed outside the repository (Python script: `pdftoppm -r 40 -gray` rendering, then
`skimage.metrics.structural_similarity` page by page).
