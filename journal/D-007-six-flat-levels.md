# D-007 — Six cumulative levels, each one flat `Profile`; lossless passes at every level

| | |
|---|---|
| **Identifier** | D-007 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-28, 2026-09-30 and 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective), revised |
| **Commits** | `41f2fdb` (Low/Medium/High), `7d41753` (experimental `extreme-safe`/`extreme`/`extreme-max`, CLI-only, `--tune`), `32eca47` (one flat profile, six levels), `a896ce3` (low/medium rebalanced) |

## 1. Context

The engine has a dozen knobs (target DPI, trigger, JPEG quality range, SSIM target, caps, and a switch
per pass). Users should pick one word, the same in the CLI, the app, the Finder service and the web
page, and a developer should be able to try a variant without recompiling. The question is how many
levels, what each one may do, and how the knobs are organised in code.

## 2. Options considered

**A** — Three levels (low/medium/high), as in iLovePDF-style tools. What the first commit shipped,
each a DPI target and a fixed JPEG quality.

**B** — Keep three "regular" levels and add experimental ones off to the side, with their extra
passes behind an `Experimental` sub-struct. What `7d41753` did: three CLI-only levels, not in
`Level::ALL`, "so the app is unchanged".

**C** — One ladder of six levels, each *building on the previous one*, every level a flat `Profile`
literal spread from a single `Profile::BASE`, and the same list in every front end.

## 3. What the assistant proposed

**B** first, to try the new passes without touching what users had; then **C** in `32eca47`, once the
measurements showed the "experimental" lossless passes were pure gain: "20–45 % smaller at
lossless/low/medium on real documents, same SSIM".

## 4. What we decided, and why

**C**: `lossless`, `low`, `medium` (default), `high`, `extreme`, `extreme-max`, in `level.rs`.

* **`Profile::BASE`** turns on everything lossless (deep dedup, font merging, Type 1 → CFF, CFF
  subsetting, D-009) and nothing that changes geometry or look. Each level is
  `Profile { …, ..Profile::BASE }`: one place to read what a level does.
* **Lossless passes at every level**, including `lossless`: on the slide deck, `medium` drops from
  19.6 MB to 12.9 MB with them, for a few seconds at most.
* **Lossy image tricks at every lossy level** (gray detection, palettes, opaque-SMask dropping, SSIM
  quality, D-008); **look-changing passes only at `extreme*`** (D-011).
* **Roughly halving at each step** is the design target (receipt scan: `low` 118 KB, `medium` 56 KB,
  `high` 27 KB).
* `extreme-safe` became `high`. The old fixed-quality `high` is gone.
* **`Level::summary()`** is the one-line description `--help` prints; the app and web page have
  translated copies in `app/ui/i18n.js` (D-024).
* **`--tune key=value`** (CLI, repeatable) overrides any knob and **`--suffix`** names the output, so
  variants can be compared side by side without a rebuild; `PDFSHRINK_DEBUG=1` prints per-phase timings
  and the quality search.

## 5. What would make us change our minds

* If users could not tell adjacent levels apart (sizes too close on typical files), the ladder should
  shrink rather than gain knobs.
* If a pass needed to be *on* at a level below one where it is *off*, the "each builds on the
  previous" rule would break; that is a sign the pass belongs to a different axis (e.g. a separate
  "screen only" switch for CMYK → RGB) rather than to the ladder.

## Revision (2026-10-05, `a896ce3`) — low and medium rebalanced

`low` and `medium` used a fixed quality and late triggers (low: 300 dpi above 450; medium: above 225),
so a 200 dpi scan was left at full resolution by both (148 and 116 KB) and then dropped straight to
96 dpi at `high`. They now search the quality on SSIM like the higher levels (`low` ≥ 0.995 in
q65–85, `medium` ≥ 0.99 in q50–75) and downsample sooner (`low`: 200 dpi above 250; `medium`: 150 dpi
above 180). Corpus of 227 files: `low` 392 → 274 MB, `medium` 218 → 200 MB, no file bigger at any
level; `low` and `medium` take ~2× the time.
