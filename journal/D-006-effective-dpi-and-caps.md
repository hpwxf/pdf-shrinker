# D-006 — Image resolution from the on-page placement, backed by pixel caps; always re-encode

| | |
|---|---|
| **Identifier** | D-006 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-28 and 2026-09-30) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (`placement.rs`, DPI trigger), `210bb07` (always re-encode, `max_dimension`), `7d41753` (`page_px`, per-page op budget, `visible` area) |

## 1. Context

Most of a PDF's weight is images, and most of the savings come from lowering their resolution. "Too
many pixels" only makes sense relative to the size an image is **drawn** at: a 3000 px image is fine
on an A0 poster and absurd as a thumbnail. PDF does not store that size; it results from the content
stream (the CTM at each `Do`). Images can also be drawn several times, from nested forms, or not be
reached by any page at all.

## 2. Options considered

**A** — Cap pixel dimensions only (e.g. longest side ≤ N px). No content parsing; blind to the page
size, so either too aggressive on posters or too lax on thumbnails.

**B** — Effective DPI from the placement: walk every page's content (and nested Form XObjects),
track the CTM through `q`/`Q`/`cm`, measure the transformed unit square at each `Do`, keep the most
demanding placement. Downsample only above `target_dpi × trigger_ratio`. What Ghostscript and
iLovePDF do.

**C** — B, plus absolute and page-relative pixel caps as backstops where the DPI is misleading, and
re-encoding decoupled from downsampling.

## 3. What the assistant proposed

**B** in the first commit, then **C** in two steps, each argued in its commit:

* `210bb07`: on a 103 MB Keynote deck, images drawn oversized and then clipped to the page ("full-bleed
  background") fooled the CTM into reporting a low DPI, "so those images were skipped entirely
  instead of being recompressed. Ghostscript has the same blind spot." Two fixes: re-encode every
  eligible image "regardless of DPI (a raw/Flate bitmap shrinks a lot from that alone)", and add
  `max_dimension` (4200/3000/2000 px) "as the backstop for the clipping case". `high` went from
  16.8 MB to 14.7 MB.
* `7d41753`: `page_px`, because DPI "is meaningless for a slide exported at 1920×1080 points", and a
  per-page op budget (a document-wide one left images on later pages to the fallback).

## 4. What we decided, and why

**C**. In `placement.rs` and `image_ops.rs`:

* **Effective DPI** from the CTM walk; when an image is placed several times, the smallest DPI (its
  largest placement) wins. An image the walk never reaches falls back to the document's largest page
  size — the conservative choice, least likely to downsample wrongly. The op budget is per page.
* **Trigger** (`target_dpi × trigger_ratio`, e.g. 150 dpi above 180 at `medium`) so an image just
  above target is not resampled for a negligible gain.
* **`max_dimension`** on the longest side, at every lossy level, independent of placement.
* **`page_px`** at `extreme*` only (D-011): pixels across the page's **displayed width** (height if
  rotated 90°), not its longest side — a 1512×14400 pt web-page export is read at its width.
* **Re-encoding is not gated by resolution**: every eligible image is re-encoded (JPEG or palette,
  D-008, D-010), and downsampled only when one of the three limits is crossed; whichever wants the
  smallest result wins. Resampling is Lanczos3.

The walk also records each image's `visible` area (union of placements ∩ MediaBox), which cropping
uses (D-011).

## 5. What would make us change our minds

* If `max_dimension` started hurting legitimate large-format documents (maps, posters) at `medium`:
  it would need to become page-relative too, rather than higher.
* If clipping were tracked properly (intersecting the placement with the clip path), the caps could
  relax; `visible` already does part of that for cropping.
