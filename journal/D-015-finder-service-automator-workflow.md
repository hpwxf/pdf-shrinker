# D-015 — Finder integration: an Automator `.workflow` service calling the bundled CLI, not an `.appex`

| | |
|---|---|
| **Identifier** | D-015 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-28 and 2026-10-01) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (`packaging/PdfShrinker.workflow`, `integration.rs`, "Quick Action"), `bc64985` (install fixes, renamed "Finder service") |

## 1. Context

"Right-click a PDF, compress it" without opening the app was part of the brief. The first version
shipped what it called a **Quick Action**: an Automator Service bundle, written by hand (no GUI to
round-trip it through Automator.app), installed into `~/Library/Services` with the path of the CLI
filled in, plus a `pdfshrink` symlink on the PATH. On 2026-10-01 "Install integrations" was found
broken in four ways, and the menu entry never appeared under *Quick Actions*.

## 2. Options considered

**A** — Keep the Automator `.workflow`: a template filled in by `integration::install_finder_service`,
running `pdfshrink --notify "$@"`. No extra build target; it lands in the **Services** submenu only.

**B** — An Action extension (`.appex`, `NSExtensionPointIdentifier` `com.apple.ui-services`) in
`Contents/PlugIns/`, the way ImageOptim does it. A real *Quick Actions* entry; a separate Swift/AppKit
target outside cargo, built and signed by `build-dmg.sh`, and in practice a Developer ID (D-017).

**C** — A Shortcuts action. Also feeds *Quick Actions*; needs App Intents, i.e. Swift as well.

## 3. What the assistant proposed

**A**, after finding (`bc64985`) that "as of macOS 26 no Automator .workflow appears in either"
*Quick Actions* or System Settings' Finder extensions list, "whoever wrote it" — so the entry was
renamed rather than chased. B's cost is recorded in `CLAUDE.md` and the README.

## 4. What we decided, and why

**A**, called a **Finder service** everywhere the user reads it (`--finder-service`, `--quick-action`
kept as an alias). The four fixes of `bc64985` became rules:

* **The workflow runs the bundled CLI**: `resolve_exec_path()` points at `Contents/MacOS/pdfshrink`
  (the `externalBin` sidecar) inside `PdfShrinker.app`, so it survives app updates. Outside a bundle it
  takes the `pdfshrink` next to the current executable and **errors** when there is none — falling
  back to `current_exe()` had made the workflow launch the GUI with `--notify` and hang.
* **The CLI symlink** goes to `/usr/local/bin` if writable, else `~/.local/bin` (`/usr/local/bin` is
  `root:wheel` on a stock macOS; a GUI app can never write there). The CLI warns when the directory
  is not on PATH; the app shows the path it actually used.
* **`--notify`** (`osascript`) tells the user a background compression happened.
* The `.workflow` template was aligned key for key with a known-good Automator-produced one, and is
  tested with `automator -i file.pdf ~/Library/Services/PdfShrinker.workflow`.

Installing writes into the user's home and possibly `/usr/local/bin`: never run from CI or an agent
without asking (`CLAUDE.md`).

## 5. What would make us change our minds

* Once a Developer ID is available (D-017), B becomes cheap enough to reconsider: a *Quick Actions*
  entry is what users look for first.
* If a future macOS dropped Automator services from the Services menu too, A would stop working
  outright.
