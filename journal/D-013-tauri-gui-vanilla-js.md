# D-013 — GUI on Tauri 2, static HTML/JS with no npm, core called directly with per-file events

| | |
|---|---|
| **Identifier** | D-013 |
| **Date** | 2026-11-20 (retrospective; decisions taken from 2026-09-28 to 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (Tauri 2 app, `RunEvent::Opened`), `4564e22` (async `compress_files`, one-line rows), `fd39940` (opened files buffered until `frontend_ready`), `1cc4310` (compare all levels), `302e544` (run in parallel) |

## 1. Context

The app has to be a real macOS citizen in one specific way: appear in Finder's **Open With** and
receive the files. macOS delivers them as an `odoc` Apple Event to the `NSApplication` delegate, both
at cold start and while running. Beyond that the UI is small: a drop zone, a level selector, a list
of files with their status.

## 2. Options considered

**A** — A native Rust GUI (egui/winit, iced). One language, no webview; but winit has no API for the
`odoc` event, so "Open With" would need patching the application delegate through `objc2`.

**B** — Tauri 2, which exposes `odoc` as `RunEvent::Opened { urls }`, with a web UI. A webview and a JS
layer, but the event is handled for us.

**C** — A Swift/SwiftUI app calling the core through FFI. The most native; a second toolchain, and
the Windows build (D-016) would be impossible.

For the web layer of B: a framework with npm and a bundler, or static HTML/CSS/JS.

## 3. What the assistant proposed

**B** with static files, for the `odoc` reason, which `CLAUDE.md` records: "Tauri 2, chosen
specifically because plain egui/winit has no API for the NSApplication odoc Apple Event". The
no-npm choice is stated (`withGlobalTauri: true` "exists specifically so app.js can call
`window.__TAURI__` directly"), not argued.

## 4. What we decided, and why

**B**, static `app/ui` (HTML, CSS, `app.js`, `i18n.js`) with **no npm, no bundler**: nothing to install
or audit, and the same files are reused as-is by the web version (D-018, D-024).

Rules that came out of use:

* **The app calls `pdfshrink-core` in-process** (D-002); the bundled CLI sidecar exists only for the
  Finder service (D-015).
* **Compression never runs on the main thread.** `compress_files` is `async` and runs the batch in
  `spawn_blocking`; as a sync command it froze the webview for the whole batch (`4564e22`). It emits
  `compress-started` then `compress-result` per file, so each row moves pending → queued → running →
  done/error as it happens.
* **Opened files are buffered** in managed state until the UI calls `frontend_ready`: on a cold "Open
  With", `RunEvent::Opened` (and argv) fired before the webview listened, and the file was lost
  (`fd39940`). The first launch's files also come from argv in `setup()`.
* **Compare all levels** runs one file at all six levels (`<name>-<level>.pdf`) with size, saving and
  fidelity per level (D-020). **Run in parallel** (off by default, more memory) spreads file × level
  jobs over rayon's pool, the same one the engine uses for images, so nested work shares it instead
  of oversubscribing (`302e544`).
* The window does not scroll or bounce; only the file list scrolls (`734c562`). `user-select: none` on
  the body, re-enabled where users need to copy (install status, version tooltip; `bc64985`,
  `8297308`).

## 5. What would make us change our minds

* If the webview became the bottleneck or the bundle size a complaint, A with an `objc2` delegate
  patch would be the way out; the core API would not change.
* If the UI grew past a few screens, a framework (and npm) could pay for itself — but it would also
  have to work in the web version without a server.
