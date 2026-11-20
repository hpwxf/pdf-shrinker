# D-016 — Platform scope: macOS on Apple Silicon first, Windows on demand, the browser for the rest

| | |
|---|---|
| **Identifier** | D-016 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-28 and 2026-09-30) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective), revised |
| **Commits** | `41f2fdb` (`aarch64-apple-darwin` only), `208345b` (Windows workflow), `3086ca1` (`RunEvent::Opened` macOS-only), `d3561a7` (web version, D-018) |

## 1. Context

The tool was born as a macOS utility (Finder service, `odoc`, `.dmg`). Each extra target costs CI
time, a packaging story and platform-specific code paths. The question is which targets are
supported, and what "supported" means for each.

## 2. Options considered

**A** — Apple Silicon only (`aarch64-apple-darwin`): no x86_64, no universal binary.

**B** — Universal macOS binaries (arm64 + x86_64). Intel Macs covered; doubled build time, a second
mozjpeg build, and nobody to test on.

**C** — Full cross-platform (macOS, Windows, Linux) as first-class targets, in CI on every push.

**D** — A, plus best-effort extra builds on demand where the code already works.

## 3. What the assistant proposed

**A** at the start ("Apple Silicon only" in the first commit), then **D** for Windows (`208345b`),
with the Windows-specific adaptations listed in the commit. Not recorded why Intel Macs were excluded.

## 4. What we decided, and why

**D**:

* **macOS arm64** is the supported platform: CI runs there (D-022), the `.dmg` targets it, the Finder
  service and the CLI symlink are macOS-only.
* **Windows x86_64** is built by `.github/workflows/windows.yml`, manual `workflow_dispatch` on master
  only: a stand-alone `pdfshrink.exe` and the Tauri app (portable exe + NSIS installer), static CRT.
  `app/src-tauri/tauri.windows.conf.json` drops the CLI sidecar and Tauri's `fileAssociations` (its
  NSIS script would take over the default PDF handler); `windows/pdf-open-with.nsh` registers the app
  as an extra "Open with" choice instead. `tauri-plugin-single-instance` forwards files from a second
  launch. "Install integrations" is hidden; "Show in Explorer" uses `explorer /select`.
* **Linux** is only a build host, for the web version (D-018).
* On macOS the PDF association is ranked `Alternate`: the app appears in "Open With" without competing
  for the default handler.

Platform code stays at the edges (`cfg` in the app and `integration.rs`); the core is
platform-neutral, which is what made Windows and wasm cheap.

## 5. What would make us change our minds

* Windows users reporting real use: then the Windows build deserves a CI job on push, not only a
  manual one.
* An Intel Mac user to test on, or a Developer ID requiring universal binaries for distribution.

## Revision (2026-11-20) — "Apple Silicon only" no longer describes the product

The README and `CLAUDE.md` still opened with "Apple Silicon only", true of the first commit but not
since the Windows build (`208345b`) and the web version (`d3561a7`, D-018), which runs in any modern
browser. The scope is unchanged; only its wording is corrected. The README now has a table of where
each front end runs and how well it is checked: macOS arm64 (CLI, app, Finder service; CI on every
push), Windows x86_64 (CLI, app; manual workflow, CLI smoke-tested), any browser (web page; manual
deployment, smoke-tested in Node), and everything else (native CLI neither built nor tested). "Apple
Silicon only" still holds for the macOS desktop builds: no x86_64 or universal `.dmg`.
