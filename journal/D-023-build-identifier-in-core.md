# D-023 — Build identifier baked into `pdfshrink-core`, read by every front end through `build_info()`

| | |
|---|---|
| **Identifier** | D-023 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-09-30) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `11533db` (`build.rs`, `version.rs`, CLI `--version`, app "ⓘ"), `8297308` (tooltip selectable), `34bbc92` (web: version, commit, JPEG encoder) |

## 1. Context

A bug report ("this file came out broken") is useless without knowing which engine produced it. Cargo's
version (`0.2.0`) does not change between commits, and three front ends (CLI, app, web) each need to
show it.

## 2. Options considered

**A** — A `build.rs` in each front end crate, each running `git` and setting its own env var.

**B** — One `build.rs` in `pdfshrink-core` that bakes the commit and a dirty-tree flag via
`cargo:rustc-env`, exposed as a function, `pdfshrink_core::build_info()`.

**C** — A dedicated build-info crate (`vergen`-style), as `Alambic` later did (`oc-buildinfo`).

## 3. What the assistant proposed

**B**, with the reason in `11533db`: "env!() only resolves within the crate that writes it, but a
plain function call works across crates, so neither front end needs its own build.rs."

## 4. What we decided, and why

**B**. The CLI prints `0.2.0 (210bb07-dirty)` for `--version`; the app shows it in a footer "ⓘ"
popover (a native `title` tooltip is silent in WKWebView, `11533db`), selectable so it can be copied
into a report (`8297308`); the web page shows "Web version 0.2.0 (abc1234)" and the JPEG encoder
actually running, both read from the loaded module (`34bbc92`, D-019). The identifier describes the
**engine**, which is what matters for an output file.

## 5. What would make us change our minds

* If a build without `.git` (a source tarball, a crates.io package) needed a meaningful identifier,
  `build.rs` would need a fallback (e.g. an env var set by the release workflow).
* If several crates needed build metadata independently, C.
