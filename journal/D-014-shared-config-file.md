# D-014 — One persisted config file, read by the CLI, the app and the Finder service

| | |
|---|---|
| **Identifier** | D-014 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-09-28) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (`config.rs`), `b067d51` (`engine` key dropped, unknown keys ignored) |

## 1. Context

The Finder service is a single menu entry with no UI (D-015): it cannot ask for a level. Yet users
want it to compress "the way I usually do". The app has a level selector; the CLI has `-l`. Something
has to say what "the default level" is, and every front end has to agree on it.

## 2. Options considered

**A** — One Finder service per level (six menu entries). No shared state; a cluttered Services menu.

**B** — Each front end keeps its own default (app in its webview storage, CLI in a flag), and the
service passes a fixed level. The service would not follow the app.

**C** — One config file in the core, `Config { default_level }`, read by `CompressOptions::default()`;
the app's "Set as default level" writes it, `pdfshrink config get/set level` reads and writes it, the
service runs the CLI without `-l`.

## 3. What the assistant proposed

**C**, in the first commit. `CLAUDE.md` gives the reason: the app's checkbox "writing here is what
makes the (single, level-less) service follow the app's chosen default".

## 4. What we decided, and why

**C**: `~/Library/Application Support/com.haveneer.pdfshrinker/config.toml` (through `directories`,
behind the `native` feature; the web version has none), a TOML file with the default level. It is the
**single source of truth** for "the default"; UI-only preferences (language, "run in parallel") stay
in the webview's `localStorage` because nothing else needs them.

Unknown keys are ignored when loading, so removing a key never breaks an older file: when the
`engine` key went away with Ghostscript (D-004), existing configs still loaded (covered by a test).

## 5. What would make us change our minds

* If the config grew per-front-end settings, it would need sections rather than more top-level keys,
  so one front end's preference never changes another's behaviour by accident.
* If the Finder service ever became an `.appex` with its own UI (D-015), per-invocation level choice
  would make the shared default less central.
