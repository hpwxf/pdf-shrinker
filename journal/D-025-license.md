# D-025 — License: MIT at first, never written down; now `MIT OR Apache-2.0`

| | |
|---|---|
| **Identifier** | D-025 |
| **Date** | 2026-11-20 (retrospective; first decision taken on 2026-09-28, revised on 2026-11-20) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective), revised |
| **Commits** | `41f2fdb` (`license = "MIT"` in `[workspace.package]`, "MIT." in the README), commit adding `LICENSE-MIT` and `LICENSE-APACHE` (revision of 2026-11-20) |

## 1. Context

Until the journal was opened, the license appeared in two places only: `license = "MIT"` in the
workspace `Cargo.toml` (and in `app/src-tauri/Cargo.toml`) and one line in the README. There was no
license file. It also constrains dependencies: the Ghostscript engine (AGPL, shelled out to rather
than linked) was one of the reasons for its removal (D-004), and MuPDF, an obvious PDF library, is
AGPL too (D-003).

## 2. Options considered

**A** — MIT alone.

**B** — `MIT OR Apache-2.0`, the Rust ecosystem's usual dual license: MIT keeps compatibility with
GPLv2, Apache-2.0 adds an explicit patent grant (§3) and says contributions come under the same
terms (§5). The licensee chooses, so it never takes anything away compared with A.

**C** — A copyleft license (GPL/AGPL), which would have allowed linking MuPDF or Ghostscript.

## 3. What the assistant proposed

For the first decision, not recorded at the time. On 2026-11-20, asked what dual licensing would
bring: little today (a single author with no patents, so §3 protects against nobody; users of an app
do not redistribute its code), but it costs almost nothing while there is a single author, matches
the Rust convention and `Alambic`, and keeps open publishing `pdfshrink-core` on crates.io and
accepting contributions. Recommendation: B now, since relicensing gets hard once others contribute.

## 4. What we decided, and why

**A** at first, as written in the manifests; then **B**, by the user's decision on 2026-11-20 (see
revision). Recorded here so that adding a dependency is checked against it: everything linked so far
is permissive (per `cargo metadata`: `mozjpeg-sys` IJG AND Zlib AND BSD-3-Clause, `jpeg-encoder`
(MIT OR Apache-2.0) AND IJG, `lopdf` and `fax` MIT, `zopfli` Apache-2.0, `hayro-jpeg2000`,
`jpeg-decoder`, `image`, `flate2` and Tauri MIT OR Apache-2.0).

## 5. What would make us change our minds

* An AGPL library becoming the only way to do something essential (repair, JBIG2): that would be a
  license decision first, not a technical one.
* Once external contributions exist, any further change needs every contributor's agreement: the
  choice is effectively frozen from then on.

## Revision (2026-11-20) — license files, and `MIT OR Apache-2.0`

At the user's request, the repository is now dual licensed:

* `LICENSE-MIT` (standard MIT text, `Copyright (c) 2026 Pascal Havé`, the author listed in
  `[workspace.package]`) and `LICENSE-APACHE` (the full Apache License 2.0 text, as distributed with
  most Rust crates);
* `license = "MIT OR Apache-2.0"` in `[workspace.package]` and in `app/src-tauri/Cargo.toml`;
* the README's License section uses the usual Rust wording, including the contribution clause.

Still open: third-party notices in the shipped `.dmg`, Windows installer and web page (IJG for
mozjpeg and `jpeg-encoder`, Apache-2.0 for `zopfli`).
