# D-008 — JPEG quality searched per image on SSIM; fixed mid-range quality for scans

| | |
|---|---|
| **Identifier** | D-008 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-30 and 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective), revised |
| **Commits** | `7d41753` (SSIM search, scan detection, at the experimental levels), `32eca47` (`high`), `a896ce3` (all lossy levels, scans' fixed quality everywhere) |

## 1. Context

A fixed JPEG quality is wrong in both directions: a flat screenshot survives q40, a fine texture shows
blocks at q55. The first version used one quality per level. The question is how to choose the quality
per image, at what cost, and what to do with images where the usual criterion misleads.

## 2. Options considered

**A** — Fixed quality per level. Fast and predictable; over-spends on easy images and damages hard
ones.

**B** — Binary search, per image, for the lowest quality in the level's range whose SSIM against the
(already resized) source stays above the level's threshold. Several encodes per image.

**C** — A perceptual metric heavier than SSIM (butteraugli, SSIMULACRA). Closer to the eye, much
slower, extra dependencies.

## 3. What the assistant proposed

**B**, introduced with the experimental levels (`7d41753`: "per-image JPEG quality from an SSIM search
(luma + chroma guard)"), then extended to `high` (`32eca47`) and to `low`/`medium` (`a896ce3`). C was
not recorded as considered.

## 4. What we decided, and why

**B**, with two refinements found on real documents:

* **The score** is the **luma** SSIM, capped by the worst RGB channel's SSIM + 0.03. Luma drives the
  choice; damage to a thin coloured line (from 4:2:0 chroma subsampling) still vetoes a quality. CMYK
  uses the mean of four channels, the worst one as a guard.
* **Scanned pages are excluded from the search**. SSIM rewards reproducing scanner noise and earlier
  JPEG artefacts, which pushed scans to the top of the range for no visible benefit. A page counts as a
  scan when one image spans ≥ 80 % of the page width and ≥ 60 % of its pixels are light, low-saturation
  "paper" whose median tone is below 254 (screenshots and born-digital pages have exactly 255). Scans
  get a fixed mid-range quality: 75 / 62 / 55 / 50 / 42 from `low` to `extreme-max`.

Thresholds per level: `low` ≥ 0.995 (q65–85), `medium` ≥ 0.99 (q50–75), `high` ≥ 0.985 (q40–70),
`extreme` ≥ 0.975 (q35–65), `extreme-max` ≥ 0.96 (q30–55).

Consequence accepted: a fine texture can get a **higher** quality than the old fixed one (the slide
deck's rust texture: 525 KB at `high` vs 399 KB at the old `high`), and the run time grows (slide deck:
5 s → 43 s at `high`). The quality per byte is the point: `high` is both smaller and closer to the
original than iLovePDF "recommended" on the deck (9.1 vs 13.5 MB, worst page 0.949 vs 0.860).

## 5. What would make us change our minds

* If visible artefacts showed up on images that pass the threshold (SSIM is blind to some ringing
  around text): first raise the chroma guard or add an edge term, then consider C.
* If run time became the complaint at `low`/`medium`: cap the number of search steps, or skip the
  search for small images.
* The scan detector is calibrated on a handful of documents (`docs/compression-strategies.md` §5): a
  false positive on a light photo would give it a fixed quality it may not deserve.
