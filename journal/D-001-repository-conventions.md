# D-001 — Repository conventions: `journal/`, `CLAUDE.md` as the agents' reference, English, definition of "done"

| | |
|---|---|
| **Identifier** | D-001 |
| **Date** | 2026-11-20 |
| **Author** | Pascal Havé |
| **Status** | adopted |
| **Commits** | journal opening commit (D-001 to D-025) |

## 1. Context

The repository has 35 commits written over successive assistant sessions (several Claude models, from
2026-09-28 to 2026-10-05) that share no context. Continuity rests on four supports:

* `CLAUDE.md` — tracked by git, 21 KB: commands, architecture, and a good deal of "why" written inline
  (why Tauri, why an Automator workflow lands under *Services*, why zune-jpeg is not used);
* `docs/compression-strategies.md` — the strategies per level and the measurements against iLovePDF,
  with its own "known limitations" section;
* `TODO.md` — image kinds still untouched and other ideas;
* commit messages, most of them argued, with numbers.

None of them answers, in one place, "why this choice, against what, and what would call it into
question". The rationale that exists is scattered across `CLAUDE.md` paragraphs and commit bodies,
and parts of it have already gone stale (see open points). The sibling projects `slidekit` and
`Alambic` have already paid for this question with a `journal/` of ADRs.

## 2. Options considered

**A** — Keep the existing supports. Nothing to write, but `CLAUDE.md` is a reference for *what is*,
not a record of *what was set aside*: when a decision is revisited (the Ghostscript engine, the
experimental levels), the old reasoning vanishes with the text that is rewritten.

**B** — Grow `docs/compression-strategies.md` into a design document that also records decisions. It
already mixes description and measurement; adding trade-offs about the GUI, distribution or CI would
make it the wrong place for most of them.

**C** — The `slidekit`/`Alambic` journal: one file per decision, five mandatory fields, retrospective
entries reconstructed and marked as such, a precedence rule for new ones. `CLAUDE.md` and the
strategies document stay what they are.

## 3. What the assistant proposed

**C**, reusing the format of `slidekit/journal/README.md` and `Alambic/journal/README.md` (five fields,
"retrospective entries" section, precedence rule, "open points" section), in English since the user
asked for it and the whole repository is in English, and adding to the README a list of questions
waiting for an entry, so the journal can serve as the starting point for the next developments.

## 4. What we decided, and why

**C**, with these conventions:

* **Documents**: `CLAUDE.md` stays the reference for agents and contributors (commands, architecture);
  there is no `AGENTS.md` and no `PLAN.md` — the project has no staged plan to index.
  `docs/compression-strategies.md` holds strategies and measurements; `TODO.md` holds work items;
  `journal/` holds trade-offs. A paragraph of `CLAUDE.md` that explains *why* may stay (agents read it
  first), but the journal entry is the one that lists the alternatives.
* **Language**: English everywhere — code, comments, commits, `README.md`, `CLAUDE.md`, `docs/`,
  `journal/`. The only French in the repository is the app's and web page's French strings (D-024).
* **Definition of "done"**: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  --locked -- -D warnings` and `cargo test --workspace --locked` pass locally, with the CLI sidecar
  staged as CI does — i.e. **exactly what `ci.yml` replays** (D-022). A change to the engine's output
  also states its effect on the documents of `docs/compression-strategies.md` §4 or on the corpus
  (D-021).
* **Recorded deviations**: when the implementation contradicts a recorded decision, `CLAUDE.md`,
  `docs/` and the entry are corrected in the same commit.

## 5. What would make us change our minds

* A decision entering a commit without an entry, or `CLAUDE.md` and the journal contradicting each
  other: the convention is not being kept and should be simplified rather than left to rot.
* If `CLAUDE.md` keeps growing (it already carries rationale paragraphs), its "why" parts could be cut
  down to links into the journal.

## Open points (review of 2026-11-20)

Documentation that has drifted from the code, found while reconstructing the entries:

* `TODO.md` ("Other ideas") still lists merging simple TrueType subsets as open, and
  `docs/compression-strategies.md` §4.2 says "simple TrueType fonts … that neither tool merges";
  `247e56d` merged them (§3.3 of the same document says so).
* The module comment of `crates/pdfshrink-core/src/image_ops.rs` still says JPX, CCITT, indexed,
  CMYK and image masks are left alone; `fe6a6d2` and `31ea3cd` handle them (D-010).
* `CLAUDE.md`'s list of Tauri commands omits `frontend_ready` and `get_build_info`.
