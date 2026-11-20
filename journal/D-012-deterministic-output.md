# D-012 — Byte-identical output from one run to the next

| | |
|---|---|
| **Identifier** | D-012 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `3801a31` |

## 1. Context

Two runs of the same file at the same level gave files of the same content but 1 to 6 bytes apart.
Cause (`3801a31`): the TrueType and Type 1 subset merges grouped programs in a `HashMap` keyed by base
font name and created the merged objects while iterating it; `HashMap` iteration order depends on the
per-process random seed, so new objects got their numbers in a different order and the object streams
deflated differently. Harmless for users, but it makes any comparison of two builds (did this change
the output?) noisy.

## 2. Options considered

**A** — Accept it: content is identical, sizes differ by a few bytes.

**B** — Make the output deterministic: ordered collections (`BTreeMap`, sorted vectors) wherever
iteration order decides object numbering or output order.

## 3. What the assistant proposed

**B**, with the check that settled it: "two runs of each of the 225 corpus files at `high`, same md5"
(`3801a31`).

## 4. What we decided, and why

**B**. A pass that creates objects iterates in a stable order. Determinism makes regression checks
trivial (same input, same md5), and the parallel image phase does not break it: images are *planned*
in parallel from cloned streams and *applied* sequentially (`placement.rs`, `image_ops.rs`).

## 5. What would make us change our minds

Nothing foreseeable: a pass that needed nondeterminism would be a bug. The open question is
enforcement, not the rule.

## Open points (review of 2026-11-20)

* Determinism is checked by hand on the corpus, not by a test. A test compressing a fixture twice
  (with fonts to merge) and comparing bytes would catch a new `HashMap` iteration — 29 `HashMap`
  uses remain in `crates/pdfshrink-core/src`, each fine only as long as nothing iterates it to
  create objects.
* Output is deterministic per build and per JPEG encoder, not across them: the web version and
  `--tune jpeg=rust` produce different files by design (D-019).
