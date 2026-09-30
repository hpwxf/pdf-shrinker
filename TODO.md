# TODO

## Image kinds the engine leaves untouched

Generate test PDFs for all of these with `scripts/make-test-pdfs.sh [OUT_DIR]` (dev-only tools:
ImageMagick, OpenJPEG, img2pdf, qpdf, Pillow). Current behaviour on its output: only the control
page (8-bit RGB JPEG) shrinks; every other image comes out unchanged.

| File | Image | `medium` today | `extreme` today |
|---|---|---|---|
| `cmyk-dct.pdf` | 2400×1600 CMYK JPEG (Adobe, inverted) | 2.9 MB → 2.9 MB | same |
| `cmyk-flate.pdf` | 2400×1600 DeviceCMYK, Flate | 9.1 MB → 9.1 MB | same |
| `jpx-rgb.pdf` | 2400×1600 RGB JPEG 2000 | 564 KB → 563 KB | same |
| `jpx-gray.pdf` | 2400×1600 gray JPEG 2000 | 189 KB → 188 KB | same |
| `ccitt-g4.pdf` | 2400×1600 1-bit CCITT G4 | 32 KB → 31 KB | same |
| `rgb16-flate.pdf` | 2400×1600 16-bit RGB, Flate | 7.4 MB → 7.4 MB | same |
| `control-rgb-dct.pdf` | 2400×1600 RGB JPEG (control) | 1.2 MB → 69 KB | 1.2 MB → 26 KB |
| `all-kinds.pdf` | all of the above, one per page | | |

### CMYK (priority)

Common in print-ready PDFs (InDesign, Illustrator exports, magazines, brochures).

- [ ] Decode `DeviceCMYK` and `ICCBased` with `/N 4`, raw/Flate and JPEG. CMYK JPEGs written with
  an Adobe APP14 marker are inverted: honour the `/Decode [1 0 1 0 1 0 1 0]` array (img2pdf and
  Photoshop both produce such files).
- [ ] Re-encode **as CMYK JPEG** (mozjpeg supports `JCS_CMYK`) with the usual downsampling and
  quality logic, keeping the colour space: converting to RGB would change colours and break
  print workflows. The SSIM search needs a CMYK-aware comparison (per channel, or after a naive
  conversion to RGB for the metric only).
- [ ] Optional, experimental levels only: convert to RGB when the document is clearly meant for the
  screen (no output intent, no spot colours), for the extra size gain. Needs a CMYK → RGB
  conversion (naive formula, or the embedded ICC profile via a CMS such as `lcms2`).
- [ ] Tests: synthesize CMYK raw/Flate and CMYK JPEG images in-test (mozjpeg can write CMYK), as
  the existing tests do for RGB.

### JPEG 2000 (`JPXDecode`)

Found in scans from some copiers, archival PDFs (PDF/A), and some Acrobat "optimize" outputs.

- [ ] Add a JPEG 2000 decoder. Options: the pure-Rust `hayro-jpeg2000` decoder, or OpenJPEG through
  bindings (`jpeg2k`, `openjpeg-sys`), which adds a C dependency. Must be sandboxed like the
  mozjpeg encoder (`catch_unwind`), since JPX decoders see hostile input.
- [ ] Handle the colour space coming from inside the codestream when the image dictionary has no
  `/ColorSpace`, and `/SMaskInData` (alpha embedded in the JPX, to be split into a PDF `/SMask`).
- [ ] Re-encode as JPEG (or palette / lossless when that's smaller), with the usual resampling.
  Keep the original when the JPX is already smaller: modern JPX is efficient, so the gain comes
  mostly from downsampling.
- [ ] Tests: decoding needs real JPX bytes. Either a dev-dependency encoder used in-test, or a tiny
  codestream (a few hundred bytes) produced once by `opj_compress` and embedded in the test
  source as a byte array.

### Also untouched, lower priority

- [ ] 16-bit images: reduce to 8 bits, then the normal path.
- [ ] Bi-level images (CCITT G3/G4, JBIG2, 1-bit Flate): downsample, or re-encode 1-bit Flate as
  CCITT G4 (JBIG2 generic region encoding would compress better but is a big job).
- [ ] Already-indexed (palette) images: currently skipped entirely.

## Other ideas (see `docs/compression-strategies.md` §5)

- [ ] Merge CFF subsets (`FontFile3`, e.g. cairo's `f-0-0` fonts in matplotlib figures) and simple
  TrueType subsets: the remaining font gap with iLovePDF on the LaTeX thesis.
- [ ] Scans: bi-level mode for pages without colour, or a mixed-raster split (sharp text mask +
  low-resolution colour background).
- [ ] Rewrite page content and vector figures (round numbers, drop useless operators).
- [ ] JPEG mode for "soft" transparency masks (shadows, gradients).
- [ ] Repair of broken PDFs that lopdf refuses to open (Ghostscript used to handle some of these).
