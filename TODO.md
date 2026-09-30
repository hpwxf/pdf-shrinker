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
| `bilevel-flate.pdf` | 2400×1600 1-bit, Flate | untouched | unchanged¹ | 26 → 24 KB |
| `ccitt-g4.pdf` | 2400×1600 1-bit CCITT G4 | untouched | untouched | untouched |
| `control-rgb-dct.pdf` | RGB JPEG (control) | 1.2 MB → 69 KB | same | → 26 KB |

¹ G4 is tried but isn't smaller than Flate on this image (large flat shapes); on a real
300 dpi text page G4 wins by ~40 %.

Rendering checked against the originals with poppler and Ghostscript (colour renders, mean
per-channel difference in line with the RGB control; no inversion), and covered by integration
tests (`tests/compression.rs`).

Done:
- [x] CMYK (Device and ICC), raw/Flate and JPEG, re-encoded as CMYK JPEG; Adobe `/Decode`
  convention handled.
- [x] JPEG 2000 decoding (pure-Rust `hayro-jpeg2000`), incl. `/SMaskInData` alpha → `/SMask`.
- [x] 16-bit images reduced to 8 bits.
- [x] Palette (`Indexed`) images decoded; lossless palette re-encoding tried.
- [x] 1-bit raw/Flate images and stencil masks → lossless CCITT G4 when smaller.

Still open:
- [ ] CMYK → RGB conversion on the experimental levels, for documents only meant for the screen
  (no output intent, no spot colours). Needs a CMS (`lcms2`) or at least the naive formula.
- [ ] Downsample over-resolved bi-level images (600 dpi scans → 300 dpi) and re-encode CCITT
  inputs; JBIG2 generic-region encoding would compress better than G4 but is a big job.
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
