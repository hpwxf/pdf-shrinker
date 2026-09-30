#!/usr/bin/env bash
# Generates PDFs exercising every image kind beyond plain 8-bit gray/RGB
# (CMYK, JPEG 2000, CCITT, 1-bit, 16-bit, indexed), for testing the engine on
# them and comparing with other tools.
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

# CMYK JPEG with an embedded ICC profile (-> ICCBased, /N 4), when macOS'
# generic CMYK profile is available.
CMYK_ICC="/System/Library/ColorSync/Profiles/Generic CMYK Profile.icc"
SRGB_ICC="/System/Library/ColorSync/Profiles/sRGB Profile.icc"
if [ -f "$CMYK_ICC" ] && [ -f "$SRGB_ICC" ]; then
  magick "$TMP/src.png" -profile "$SRGB_ICC" -profile "$CMYK_ICC" -quality 92 "$TMP/cmyk-icc.jpg"
  pdf cmyk-icc-dct.pdf "$TMP/cmyk-icc.jpg"
fi

# JPEG 2000 (JPXDecode), RGB and grayscale, lossy at ~20:1.
magick "$TMP/src.png" "$TMP/rgb.ppm"
opj_compress -i "$TMP/rgb.ppm" -o "$TMP/rgb.jp2" -r 20 >/dev/null
pdf jpx-rgb.pdf "$TMP/rgb.jp2"
magick "$TMP/src.png" -colorspace Gray -depth 8 "$TMP/gray.pgm"
opj_compress -i "$TMP/gray.pgm" -o "$TMP/gray.jp2" -r 20 >/dev/null
pdf jpx-gray.pdf "$TMP/gray.jp2"

# Palette image (Indexed, 64 colours, Flate), as GIF/PNG8 sources produce.
magick "$TMP/src.png" -colors 64 "PNG8:$TMP/indexed.png"
pdf indexed-flate.pdf "$TMP/indexed.png"

# Bi-level CCITT Group 4 (CCITTFaxDecode), as black-and-white scanners produce.
magick "$TMP/src.png" -colorspace Gray -threshold 55% -type bilevel -compress Group4 "$TMP/g4.tif"
pdf ccitt-g4.pdf "$TMP/g4.tif"

# Bi-level image stored as 1-bit Flate (what CCITT G4 would shrink). Written by
# hand: img2pdf always uses CCITT for 1-bit input.
python3 - "$TMP/src.png" "$OUT/bilevel-flate.pdf" <<'PY'
import sys, zlib
from PIL import Image
im = Image.open(sys.argv[1]).convert("L").point(lambda v: 255 if v > 140 else 0).convert("1")
w, h = im.size
data = zlib.compress(im.tobytes(), 9)  # 1 bit/pixel, rows byte-aligned, 1 = white
pw, ph = w * 72 / 300, h * 72 / 300
content = f"q {pw:.2f} 0 0 {ph:.2f} 0 0 cm /Im0 Do Q".encode()
objs = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2f} {ph:.2f}] "
    f"/Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>".encode(),
    b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
    b"<< /Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace /DeviceGray "
    b"/BitsPerComponent 1 /Filter /FlateDecode /Length %d >>\nstream\n" % (w, h, len(data))
    + data + b"\nendstream",
]
out = bytearray(b"%PDF-1.5\n")
offsets = []
for i, o in enumerate(objs, 1):
    offsets.append(len(out))
    out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
out += b"".join(b"%010d 00000 n \n" % off for off in offsets)
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
open(sys.argv[2], "wb").write(out)
PY
echo "  $OUT/bilevel-flate.pdf"

# 16 bits per component RGB (Flate).
magick "$TMP/src.png" -alpha off -depth 16 "PNG48:$TMP/rgb16.png"
pdf rgb16-flate.pdf "$TMP/rgb16.png"

# Everything in one document, plus an 8-bit RGB JPEG page the engine already
# handles, as a control.
magick "$TMP/src.png" -quality 92 "$TMP/rgb.jpg"
pdf control-rgb-dct.pdf "$TMP/rgb.jpg"
rm -f "$OUT/all-kinds.pdf"
qpdf --empty --pages $(ls "$OUT"/*.pdf | grep -v control-rgb-dct) "$OUT"/control-rgb-dct.pdf \
  -- "$OUT/all-kinds.pdf"
echo "  $OUT/all-kinds.pdf"
