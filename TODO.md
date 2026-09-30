# TODO

## Image kinds: status

Test PDFs for every case: `scripts/make-test-pdfs.sh [OUT_DIR]` (dev-only tools: ImageMagick,
OpenJPEG, img2pdf, qpdf, Pillow). Results with the current engine:

| File | Image | before | `medium` | `extreme` |
|---|---|---|---|---|
| `cmyk-dct.pdf` | 2400×1600 CMYK JPEG (Adobe, inverted) | untouched | 2.9 MB → 286 KB | → 99 KB |
| `cmyk-icc-dct.pdf` | same, ICC profile (`ICCBased`, N=4) | untouched | 2.8 MB → 326 KB | → 137 KB |
| `cmyk-flate.pdf` | 2400×1600 DeviceCMYK, Flate | untouched | 9.1 MB → 184 KB | → 67 KB |
| `jpx-rgb.pdf` | 2400×1600 RGB JPEG 2000 | untouched | 564 KB → 65 KB | → 25 KB |
| `jpx-gray.pdf` | 2400×1600 gray JPEG 2000 | untouched | 189 KB → 47 KB | → 18 KB |
| `indexed-flate.pdf` | 2400×1600 64-colour palette, Flate | untouched | 899 KB → 76 KB | → 25 KB |
| `rgb16-flate.pdf` | 2400×1600 16-bit RGB, Flate | untouched | 7.4 MB → 69 KB | → 26 KB |
| `bilevel-flate.pdf` | 2400×1600 1-bit, Flate (300 dpi) | untouched | unchanged¹ | 26 → 8.7 KB (192 dpi) |
| `ccitt-g4.pdf` | 2400×1600 1-bit CCITT G4 (300 dpi) | untouched | unchanged¹ | 30 → 8.6 KB (192 dpi) |
| `control-rgb-dct.pdf` | RGB JPEG (control) | 1.2 MB → 69 KB | same | → 26 KB |

¹ At `medium` the bi-level target is 300 dpi, so these 300 dpi images keep their resolution;
lossless G4 re-encoding is tried but isn't smaller on these synthetic images (large flat
shapes). On a real 300 dpi text page G4 beats Flate by ~40 %.

Rendering checked against the originals with poppler and Ghostscript (colour renders, mean
per-channel difference in line with the RGB control; no inversion), and covered by integration
tests (`tests/compression.rs`).

Done:
- [x] CMYK (Device and ICC), raw/Flate and JPEG, re-encoded as CMYK JPEG; Adobe `/Decode`
  convention handled.
- [x] JPEG 2000 decoding (pure-Rust `hayro-jpeg2000`), incl. `/SMaskInData` alpha → `/SMask`.
- [x] 16-bit images reduced to 8 bits.
- [x] Palette (`Indexed`) images decoded; lossless palette re-encoding tried.
- [x] 1-bit raw/Flate/CCITT G4 images and stencil masks: downsampled to 2× the colour targets
  when over-resolved, then CCITT G4 when smaller.

Still open:
- [ ] CMYK → RGB conversion at the `extreme` levels, for documents only meant for the screen
  (no output intent, no spot colours). Needs a CMS (`lcms2`) or at least the naive formula.
- [ ] JBIG2 generic-region encoding (better than G4, what iLovePDF uses at "extreme"); decoding
  JBIG2 and CCITT Group 3 inputs.
- [ ] Lab, Separation/DeviceN, and JPEG 2000 with an explicit palette colour space: still
  untouched.

## Other ideas (see `docs/compression-strategies.md` §5)

- [ ] Merge CFF subsets (`FontFile3`, e.g. cairo's `f-0-0` fonts in matplotlib figures) and simple
  TrueType subsets: the remaining font gap with iLovePDF on the LaTeX thesis.
- [ ] Scans: bi-level mode for pages without colour, or a mixed-raster split (sharp text mask +
  low-resolution colour background).
- [ ] Rewrite page content and vector figures (round numbers, drop useless operators).
- [ ] JPEG mode for "soft" transparency masks (shadows, gradients).
- [ ] Repair of broken PDFs that lopdf refuses to open (Ghostscript used to handle some of these).
