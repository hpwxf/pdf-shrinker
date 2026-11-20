# D-002 — One compression core, `pdfshrink-core`, called in-process by every front end

| | |
|---|---|
| **Identifier** | D-002 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-09-28) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (workspace, core, CLI, app, Finder service), `d3561a7` (`compress_bytes`, WebAssembly front end) |

## 1. Context

The product is meant to be reachable from wherever a PDF happens to be: a terminal, a window to drop
files into, a right-click in Finder — and, later, a web page. Each of these front ends could end up
with its own notion of "compress at medium". The risk is the classic one: a file compressed from the
Finder differs from the same file compressed in the app, and nobody knows which one the
documentation describes.

## 2. Options considered

**A** — One binary (the CLI) that every other front end shells out to. Simple to keep consistent, but
the app would parse a text protocol to show per-file progress, and a browser cannot run a process.

**B** — A library crate that holds **all** compression logic, with thin front ends that only parse
input, call one function and present the result. The CLI, the app and the WebAssembly bindings each
link it.

**C** — Separate implementations per front end (e.g. a Swift app on PDFKit, a Rust CLI). Native
look for free, and three behaviours to keep aligned by hand.

## 3. What the assistant proposed

**B**. The first commit (`41f2fdb`) describes the workspace as "shipped three ways from one Cargo
workspace sharing a single compression engine"; `CLAUDE.md` states the intent as "so behavior never
drifts between front ends". The alternatives were not recorded at the time.

## 4. What we decided, and why

**B**, with one deliberate exception:

* `compress_file(input, &CompressOptions) -> Result<Outcome>` (`compress.rs`) is the single entry point
  of the desktop front ends; `compress_bytes(&[u8], name, &Profile)` (`rust_engine.rs`) is its
  in-memory core and the only entry point of the WebAssembly build (D-018). Output naming, the
  "written only if smaller" rule (D-005) and the default level (D-014) live in the core, not in the
  front ends.
* The app calls the core **in-process**, not through its bundled CLI.
* The exception: the **Finder service** runs the bundled CLI (`pdfshrink --notify`), because an
  Automator workflow can only run a shell script (D-015). It goes through the same `compress_file`,
  so it still cannot drift.

The boundary held: per-file events in the app (`4564e22`), "compare all levels" (`1cc4310`), parallel
runs (`302e544`) and the whole web version (`d3561a7`) were added without a second implementation of
any compression step. The README can show the same `test.pdf` giving the same sizes in the CLI and the
app.

## 5. What would make us change our minds

* If a front end needed something the core cannot express without knowing about that front end (a
  cancellation token, streaming progress inside one file): the answer would be to extend the core's
  API (a callback, a `Report` field), not to move logic out of it.
* If the core's `native`/wasm split (D-018) grew so many `cfg`s that the two builds became two engines
  in one crate.
