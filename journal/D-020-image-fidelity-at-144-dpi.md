# D-020 — Image fidelity reported as luma SSIM judged at 144 dpi on the page

| | |
|---|---|
| **Identifier** | D-020 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `1cc4310` (`Profile::measure_fidelity`, `Report::image_fidelity`, app "compare all levels") |

## 1. Context

Six levels (D-007) only help if users can see what each one costs. A size is easy to show; "how much
worse does it look" is not. Page-level SSIM against a rendering needs a PDF renderer we do not ship
(D-021 uses poppler outside the repository). The engine already decodes each source image and its
re-encoded result, so an image-level score is nearly free — but at what scale should two images of
different sizes be compared?

## 2. Options considered

**A** — Report only sizes.

**B** — SSIM of each re-encoded image against its source **at the source's size** (upscale the result
back). Counts downsampling as loss even where it is invisible.

**C** — SSIM with both brought to the size the image has **on screen**: its placement at 144 dpi (a
Retina display at 100 % zoom), never above the source's own size. Downsampling and JPEG loss count
together, as seen.

## 3. What the assistant proposed

**C**, after B gave absurd scores: "Judging at the source's size made screenshots halved from 300 to
150 dpi score 0.56 for no visible change" (`1cc4310`).

## 4. What we decided, and why

**C**: `Profile::measure_fidelity` (always on in the app and the web page; `--tune fidelity=1` in the
CLI) measures luma SSIM per re-encoded image at 144 dpi on the page; images over 4 Mpx at that
resolution are judged a little more zoomed out (an A0 map of 121 Mpx took 2 s otherwise). Both sides
go through the **same** filter (box average down to twice the size, then Triangle): mismatched filters
measure themselves, not the compression. `Report::image_fidelity` gives the mean weighted by each
image's area on the page, the worst image and the count. Cost: about +6 % run time at `medium`;
output unchanged. CMYK images are not measured.

It is a guide, not a guarantee: it only sees re-encoded images, not fonts, vector content or
cropping.

## 5. What would make us change our minds

* If the score disagreed with what users see (a level scoring 0.99 but looking worse than one scoring
  0.98): first suspect the scale or the filter, then the metric (D-008 has the same open question).
* If a renderer were ever embedded (e.g. for repair, D-003), page-level SSIM would replace image-level
  SSIM as the number shown.
