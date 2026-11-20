# D-011 — Up to `high`, a page never changes beyond compression artifacts

| | |
|---|---|
| **Identifier** | D-011 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-30) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `7d41753` (cropping, `scan_whiten`, `page_px`, Zopfli, at the experimental levels), `32eca47` (kept at `extreme*` when the levels were normalised) |

## 1. Context

Some passes save a lot but change something other than image fidelity: cropping an image to its
visible part changes how it is drawn (a Form XObject wrapper), whitening scanned paper changes every
background pixel, a page-relative cap ignores the document's own resolution, Zopfli triples the run
time. The question is where on the ladder (D-007) each one is allowed.

## 2. Options considered

**A** — Use each pass wherever it saves bytes, from `low` up.

**B** — Draw a line: up to `high`, the only visible change is the one JPEG and downsampling cause;
geometry-, tone- or time-changing passes only at `extreme` and `extreme-max`.

## 3. What the assistant proposed

**B**, from the moment these passes were introduced as experimental levels (`7d41753`), and kept when
the levels were normalised (`32eca47`: "Zopfli, crop, page-relative cap and scan whitening stay at
extreme/extreme-max"). The rendering argument for cropping is in `docs/compression-strategies.md`
§3.5.

## 4. What we decided, and why

**B**:

* **Cropping** (`extreme*`): an image overflowing the page (a 3:2 photo on a 16:9 slide: ~15 % of its
  pixels never visible) is cropped to the union of its placements ∩ MediaBox + 2 px. The cropped image
  is drawn by a small Form XObject that **takes over the image's object number**, so no page content
  is rewritten. Only when every reference to the image was seen by a complete walk, and the image has
  no `/Mask`, no `/OC`, no structure tagging and no shared soft mask. Kept out of the lower levels
  because poppler smooths an image drawn through a Form slightly differently when zoomed far out.
* **Paper whitening** (`extreme*`): a linear levels stretch mapping the detected paper tone (median
  − 20) to white on scans (D-008). About 25 % smaller at equal quality; linear rather than a
  threshold, so faint pencil strokes get lighter but never vanish; colours kept. It lowers SSIM on
  purpose, which is why it is not used below `extreme`.
* **Page-relative cap** (`page_px`, `extreme*`, D-006).
* **Zopfli** on non-image streams (`extreme*`): 5–8 % on those streams for ~3× the run time (slide
  deck: 45 s instead of 14 s).

## 5. What would make us change our minds

* If cropping's rendering difference were shown to be invisible in the viewers users actually use
  (Preview, Acrobat, Chrome), it could move down to `high`, where it would be a large gain on slide
  exports.
* If Zopfli became cheap enough (parallel per stream, or a faster equivalent), it would be a lossless
  pass and belong in `Profile::BASE`.
