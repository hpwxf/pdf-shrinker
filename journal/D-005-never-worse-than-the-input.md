# D-005 — Never worse than the input: original untouched, written only if smaller, page count checked

| | |
|---|---|
| **Identifier** | D-005 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-09-28) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (output naming, `NotSmaller`, page-count check, exit code 2) |

## 1. Context

A compressor runs on files users care about, often from a right-click with no window to show what
happened (D-015). Several failures are possible: the output is bigger (an already optimised file), the
rewrite loses pages or objects, the process dies halfway through writing, the user runs it twice. The
question is what guarantees the tool gives *before* any question of compression ratio.

## 2. Options considered

**A** — Overwrite the input in place (what some "optimise" tools do). One file to manage; any bug is
destructive.

**B** — Always write `name-compressed.pdf`, whatever the result.

**C** — A set of guarantees enforced in the core: never touch the original; write into a
same-directory temp file and rename only on success; keep the result only if it is strictly smaller;
reload the output and check the page count; never overwrite an earlier output.

## 3. What the assistant proposed

**C**, in the first commit. Not recorded at the time which alternatives were weighed.

## 4. What we decided, and why

**C**, at two levels:

* **In `compress_bytes`** (every front end, web included): the output is saved as PDF 1.5 with object
  streams, **reloaded**, and its page count compared with the input's; a mismatch discards it
  (`PageCountMismatch`). Encrypted files are refused (`Encrypted`) rather than rewritten.
* **In `compress_file`** (desktop): output `<name>-compressed.pdf`, then `-compressed-2`, `-3`… if
  taken; written to `<output>.pdfshrink-tmp` next to it and renamed into place only if
  `output_size < input_size`, otherwise deleted and `Outcome::NotSmaller` returned. The CLI turns
  `NotSmaller` into exit code `2`, checked after all files so a mix of outcomes still surfaces the
  most severe code.

The same rule applies **inside** the engine, image by image: a re-encoded image replaces the original
only if its bytes are smaller (`image_ops.rs`), and a CFF program is rewritten only if that helps
(`60564cf`). "Smaller or untouched" is the invariant at every scale, which is why no level can make a
file bigger (`a896ce3`: "no file bigger at any level" on the 227-file corpus).

## 5. What would make us change our minds

* If a pass were worth keeping even when the file grows (e.g. a repair step, D-003): `NotSmaller`
  would need a reason, not just a size.
* The page-count check is a weak proof of integrity. If a bug ever lost content while keeping pages
  (a dropped font, a broken image), the check would need strengthening (e.g. render-compare in tests,
  D-021) rather than more trust in it.

The web version applies the same rule on its side: `compress_bytes` deliberately does not compare
sizes, and `web/app.js` turns `outputSize >= inputSize` into a "not smaller" row with nothing to
download.

## Open points (review of 2026-11-20)

* The size rule exists twice (`compress_file` and `web/app.js`). A `compress_bytes` variant returning
  an `Outcome`-like enum would put it back in the core alone (D-002).
