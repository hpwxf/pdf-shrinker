# D-027 — Flattening annotations: appearance streams drawn into the page, opt-in, every front end

| | |
|---|---|
| **Identifier** | D-027 |
| **Date** | 2026-11-20 |
| **Author** | Pascal Havé |
| **Status** | adopted |
| **Commits** | this entry's commit (`flatten.rs`, `Profile::flatten_annotations`, `--flatten`, app and web checkboxes) |

## 1. Context

A form filled in macOS Preview (one page, 32 `/FreeText` annotations and an empty signature
`/Widget`) keeps every entry as a live annotation: whoever receives the file can move, edit or delete
them. The user wants to "flatten" those edits so they become part of the page, **without**
rasterizing it: the text must stay selectable and searchable. Each annotation already carries its
look as an appearance stream (`/AP /N`), a Form XObject with real text operators (`BT … TJ`) and an
embedded TrueType subset; the specification (ISO 32000 §12.5.5) says how that form maps onto the
annotation's `/Rect`. What it does not say is which annotations to flatten, where in the product the
feature lives, or what happens to a file that does not get smaller.

## 2. Options considered

**How to flatten**

**A** — Rasterize each page with its annotations (what "flatten" means in some tools). Simple to
explain; loses text selection, contradicts the request, and makes files bigger.

**B** — Draw each appearance stream into the page content with `Do` (the form becomes an XObject
resource of the page), then delete the annotation. Exactly the annotation's look, text stays text,
the form's own resources stay scoped (no font-name collisions with the page).

**C** — B, but inlining the form's operators in the page content. Saves one level of indirection;
needs resource renaming and `/Matrix`/`/BBox` clipping by hand, for no visible gain.

**D** — Shell out to `qpdf --flatten-annotations`. Proven code; an external binary, against D-003
and the web version (D-018).

**Which annotations**

All those with an appearance (qpdf's `all`), or markup and form fields only, leaving links, sticky
notes (`/Text`, whose comment would be lost) and multimedia alone. Annotations without an `/AP`:
regenerate an appearance, or leave them as they are.

**Where it lives**

A level knob, or an independent option every front end exposes; and, with it, whether the
"written only if smaller" rule (D-005) still applies.

## 3. What the assistant proposed

B, in a new `flatten.rs` pass of the core run right after loading (before pruning, so the
annotation dictionaries and Preview's `AAPL:AKExtras` blobs are dropped, and the appearance fonts and
images go through the other passes, the image placement walk included). Flatten markup annotations
and form widgets (fields removed from `/AcroForm`, `/AcroForm` dropped once empty); keep `Link`,
`Text` with its `Popup`, attachments, multimedia and `Redact`; never regenerate a missing
appearance; skip widgets when `/NeedAppearances` says their appearances are stale. Wrap the original
content in `q … Q` before drawing (unbalanced `q`/`Q` or a leftover CTM would move the
annotations). An option outside the levels (`Profile::flatten_annotations`, `--flatten`, a checkbox
in the app and the web page), and the output written even when not smaller, since the user asked for
a transformation, not only for compression. It asked the user to choose the scope, the front ends and
the "not smaller" rule.

## 4. What we decided, and why

As proposed; the user chose markup + forms, every front end, and writing the output anyway.
`Profile` carries the switch because it is what already flows from every front end to
`compress_bytes`, but no level turns it on: flattening changes what the document *is* (no longer
editable), not how hard it is compressed (D-007, D-011). The Finder service stays level-less and
option-less (D-015).

Known limits, owned: a tagged PDF's structure tree may keep an `/OBJR` pointing at a removed
annotation (the object survives pruning through that reference, harmless to viewers); hidden or
`NoView` annotations are removed without being drawn; a `NoRotate` annotation on a rotated page is
rotated around its top-left corner, like qpdf does.

## 5. What would make us change our minds

* Users wanting sticky notes or links flattened too: add a scope choice rather than widen the default.
* Files whose annotations lack an appearance (some generators rely on `/NeedAppearances`): then
  generating appearances for the simple cases (`/FreeText` with `/DA`, text fields) becomes worth it.
* A viewer rendering a flattened page differently from the annotated original.
