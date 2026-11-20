# D-018 — Web version: the same core in WebAssembly, run in the visitor's browser, on GitHub Pages

| | |
|---|---|
| **Identifier** | D-018 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `d3561a7` (`native` feature, `compress_bytes`, `pdfshrink-wasm`, `web/`), `34bbc92` (build info), `6377824` (README), `103a115` (`pages.yml`, smoke test) |

## 1. Context

A web page reaches people who will not install a `.dmg` (Windows, Linux, a borrowed machine) — the
reason iLovePDF is used at all. But iLovePDF uploads the document, and PDFs are often private. The
core is pure Rust except mozjpeg, rayon threads and a few desktop-only modules (D-003).

## 2. Options considered

**A** — A server that runs the CLI on uploaded files. Same output as the desktop; hosting, uploads of
private documents, and a service to secure and pay for.

**B** — The core compiled to WebAssembly, running in a Web Worker in the visitor's browser; the server
only serves static files. Nothing leaves the computer; no mozjpeg (C) and no threads in the browser,
so the output differs and is slower.

**C** — No web version.

## 3. What the assistant proposed

**B**, with the output difference made visible rather than hidden: "A note under the level hint warns
whenever a selected level re-encodes images, since the browser build can't use mozjpeg and its files
differ from the desktop's" (`d3561a7`). Deployment to GitHub's own Pages actions, "no third-party
host" (`103a115`).

## 4. What we decided, and why

**B**:

* **Core**: everything desktop-only sits behind the default `native` feature (`mozjpeg`, `parallel` via
  `par.rs`, lopdf's default features, `config.rs`, `compress.rs`, `integration.rs`). Without it the
  crate builds for `wasm32-unknown-unknown`. `compress_bytes` is the in-memory entry point both builds
  share (D-002). Never call `std::time::Instant::now()` unconditionally: it panics there (`PhaseLog`
  only reads the clock under `PDFSHRINK_DEBUG`).
* **Bindings**: `crates/pdfshrink-wasm` (`wasm-bindgen`): `compress(bytes, level, name)`,
  `levelDiffersFromDesktop(level)`, `buildInfo()`.
* **Page**: `web/` (static, no npm, like the app, D-013), reusing the app's `style.css` and `i18n.js`
  as-is. One module Worker, one job at a time. A Rust panic aborts the module (no unwinding in wasm),
  so the worker reports `crashed` and the page replaces it. Results are Blob downloads.
* **Honesty about the difference**: `Level::differs_from_desktop` drives a warning under the level
  hint, and the footer names the JPEG encoder actually running (D-019).
* **Deployment**: `pages.yml`, manual, master only, on Linux (no C code to build): pure-Rust core tests,
  `scripts/build-web.sh`, a Node smoke test compressing the `make_fixture` PDF at every level, then
  GitHub Pages (<https://hpwxf.github.io/pdf-shrinker/>).

## 5. What would make us change our minds

* If the size gap with the desktop (13–24 % on JPEG-heavy files) became the main complaint: a
  wasm-compiled mozjpeg (Emscripten or a `wasm32-wasi` C toolchain) is the first thing to try, before
  any server.
* If single-threaded run times made large files unusable: `wasm-bindgen-rayon` needs cross-origin
  isolation headers that GitHub Pages cannot set, so that would also mean another host.
* If the `native` gating spread into many modules, the core would need a cleaner split (D-002).
