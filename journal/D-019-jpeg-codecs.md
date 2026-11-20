# D-019 — JPEG: mozjpeg natively, pure-Rust `jpeg-encoder` otherwise; every JPEG decoded by `jpeg-decoder`

| | |
|---|---|
| **Identifier** | D-019 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-28 and 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (mozjpeg, wrapped in `catch_unwind`), `d3561a7` (`jpeg.rs`, `JpegEncoder`, `jpeg-encoder`, `jpeg-decoder`, `image` without `jpeg`) |

## 1. Context

JPEG re-encoding produces most of the savings, and the SSIM search (D-008) encodes and decodes every
image several times. mozjpeg (C) gives the smallest files at a given quality but cannot run in the
browser (D-018). On the decode side, the `image` crate's default JPEG decoder is zune-jpeg.

## 2. Options considered

Encoding: **A** mozjpeg everywhere (no web version), **B** a pure-Rust encoder everywhere (one output,
bigger files), **C** mozjpeg when compiled in, `jpeg-encoder` otherwise, selectable.

Decoding: **A** zune-jpeg through `image`, **B** `jpeg-decoder` for every JPEG, sources and candidates.

## 3. What the assistant proposed

**C** and decoding **B**, from measurements in `d3561a7`: `jpeg-encoder` files "13-24 % bigger than
mozjpeg's, about twice as fast to produce"; zune-jpeg "decodes jpeg-encoder's (valid) baseline output
as garbage and understated mozjpeg outputs' SSIM"; "the decoder swap costs nothing measurable end to
end".

## 4. What we decided, and why

* **Encoder**: `Profile::jpeg_encoder` (`--tune jpeg=mozjpeg|rust`), `JpegEncoder::DEFAULT` = mozjpeg
  when the `mozjpeg` feature is on. mozjpeg runs inside `catch_unwind`, per its own safety note.
  `jpeg-encoder` is configured to be as close as it can: baseline (its progressive mode is *bigger*),
  optimized Huffman tables, mozjpeg's ImageMagick quantisation tables, 4:2:0; CMYK unsubsampled and
  inverted so stored samples match mozjpeg's. It stays ~10–15 % bigger at equal SSIM (no trellis
  quantisation). Natively, `jpeg=rust` reproduces the web version's output (identical except CMYK).
* **Decoder**: every JPEG goes through `jpeg::decode` (`jpeg-decoder`, matches libjpeg within IDCT
  rounding). The `image` crate is built **without** its `jpeg` feature, so zune-jpeg cannot sneak back
  in; only tests and examples enable it, to write fixtures. CMYK JPEGs decode to stored samples via
  mozjpeg when present, else `jpeg::decode`, which undoes `jpeg-decoder`'s inversion and handles YCCK.

## 5. What would make us change our minds

* A pure-Rust encoder with trellis quantisation reaching mozjpeg's sizes: then B (one encoder, one
  output everywhere) and D-018's warning goes away.
* A zune-jpeg release that decodes `jpeg-encoder`'s output correctly and matches libjpeg's chroma
  upsampling: worth re-measuring, since it is faster (28 vs 44 ms on a 3000×2000 RGB decode).
