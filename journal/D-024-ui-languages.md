# D-024 — App and web page in English and French, one shared `i18n.js`; CLI in English only

| | |
|---|---|
| **Identifier** | D-024 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-28 and 2026-09-30) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `d2d666f` (README to English), `16dab5d` (EN/FR in the app), `7d41753` (hard-coded French strings in CLI and app errors switched to English), `32eca47` (translated level hints), `d3561a7` (`i18n.js` reused by the web page) |

## 1. Context

The first version was written in French (README, some CLI and error strings). Its author and first
users are French; a compressor is a tool for anyone. Front ends that show text: the CLI, the app, the
web page, and macOS notifications from the Finder service.

## 2. Options considered

**A** — English only everywhere. One set of strings.

**B** — A localisation framework (Fluent, gettext) shared by Rust and JS. Every front end translated;
infrastructure for a few dozen strings.

**C** — English as the base language; the GUI front ends in English and French through a small
hand-written `i18n.js`; the CLI (and its notifications) in English only.

## 3. What the assistant proposed

**C** (`16dab5d`: "Add EN/FR i18n to the macOS app (CLI stays English-only)"), with locale detected
from `navigator.language`, an override persisted in `localStorage`, and per-file status kept as state
so switching language re-renders past results too.

## 4. What we decided, and why

**C**. `app/ui/i18n.js` holds both dictionaries, `t(key, vars)`, `applyToDom()` for `data-i18n`
attributes, and `sizeUnits()` (Mo/Go vs MB/GB). The web page copies it as-is and keeps its own strings
under `web.*` (D-018). Everything else is English, including errors from the core, which reach the UI
verbatim.

One duplication is accepted: `Level::summary()` (for `--help`) and the level hints in `i18n.js`
describe the same thing and must be kept in sync by hand (`CLAUDE.md` says so).

## 5. What would make us change our minds

* A third language, or core error messages that need translating: then B, or at least generating the
  level hints from `Level::summary()`.
* The two level descriptions drifting apart: the first sign the duplication needs a test or a
  generator.
