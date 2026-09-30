#!/usr/bin/env bash
# Generates PDFs exercising image kinds the Rust engine doesn't handle yet
# (CMYK, JPEG 2000, CCITT, 16-bit), for developing and testing that support.
# Nothing is committed: rerun this whenever needed.
#
#   scripts/make-test-pdfs.sh [OUT_DIR]      (default: ./test-pdfs)
#
# Dev-only tools (Homebrew): imagemagick, openjpeg, img2pdf, qpdf, plus Python
# with Pillow (img2pdf depends on it).
#   brew install imagemagick openjpeg img2pdf qpdf
# img2pdf embeds images *without re-encoding them*, so each PDF carries exactly
# the filter/colour space named in its file name. Images are placed at 300 dpi,
# so every level that downsamples has something to do.
set -euo pipefail

OUT="${1:-test-pdfs}"
for tool in magick opj_compress img2pdf qpdf; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool (see header)" >&2; exit 1; }
done
mkdir -p "$OUT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Photo-like content (deterministic plasma) with sharp shapes and thin lines on
# top, so both smooth areas and edges are represented (no font needed).
magick -seed 42 -size 2400x1600 plasma:orange-navy \
  -fill white -stroke black -strokewidth 6 -draw 'rectangle 300,300 1100,700' \
  -fill '#d02020' -draw 'circle 1700,500 1700,250' \
  -stroke white -strokewidth 2 -draw 'line 200,1100 2200,1150' -draw 'line 200,1200 2200,1250' \
  -stroke black -strokewidth 1 -draw 'line 200,1300 2200,1350' \
  -depth 8 "$TMP/src.png"

pdf() { img2pdf --imgsize 300dpi --output "$OUT/$1" "$2"; echo "  $OUT/$1"; }

echo "Generating into $OUT/:"

# CMYK JPEG (Adobe APP14 marker: inverted CMYK, img2pdf adds the /Decode array).
magick "$TMP/src.png" -colorspace CMYK -quality 92 "$TMP/cmyk.jpg"
pdf cmyk-dct.pdf "$TMP/cmyk.jpg"

# CMYK raw samples, Flate-compressed (DeviceCMYK). Written by Pillow: img2pdf
# can't read ImageMagick's CMYK TIFFs.
python3 -c "from PIL import Image; Image.open('$TMP/src.png').convert('CMYK').save('$TMP/cmyk.tif', compression='tiff_lzw')"
pdf cmyk-flate.pdf "$TMP/cmyk.tif"

# JPEG 2000 (JPXDecode), RGB and grayscale, lossy at ~20:1.
magick "$TMP/src.png" "$TMP/rgb.ppm"
opj_compress -i "$TMP/rgb.ppm" -o "$TMP/rgb.jp2" -r 20 >/dev/null
pdf jpx-rgb.pdf "$TMP/rgb.jp2"
magick "$TMP/src.png" -colorspace Gray -depth 8 "$TMP/gray.pgm"
opj_compress -i "$TMP/gray.pgm" -o "$TMP/gray.jp2" -r 20 >/dev/null
pdf jpx-gray.pdf "$TMP/gray.jp2"

# Bi-level CCITT Group 4 (CCITTFaxDecode), as black-and-white scanners produce.
magick "$TMP/src.png" -colorspace Gray -threshold 55% -type bilevel -compress Group4 "$TMP/g4.tif"
pdf ccitt-g4.pdf "$TMP/g4.tif"

# 16 bits per component RGB (Flate).
magick "$TMP/src.png" -alpha off -depth 16 "PNG48:$TMP/rgb16.png"
pdf rgb16-flate.pdf "$TMP/rgb16.png"

# Everything in one document, plus an 8-bit RGB JPEG page the engine already
# handles, as a control.
magick "$TMP/src.png" -quality 92 "$TMP/rgb.jpg"
pdf control-rgb-dct.pdf "$TMP/rgb.jpg"
qpdf --empty --pages "$OUT"/cmyk-dct.pdf "$OUT"/cmyk-flate.pdf "$OUT"/jpx-rgb.pdf \
  "$OUT"/jpx-gray.pdf "$OUT"/ccitt-g4.pdf "$OUT"/rgb16-flate.pdf "$OUT"/control-rgb-dct.pdf \
  -- "$OUT/all-kinds.pdf"
echo "  $OUT/all-kinds.pdf"
