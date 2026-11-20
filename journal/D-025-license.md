# D-025 — License MIT, never written down as a decision

| | |
|---|---|
| **Identifier** | D-025 |
| **Date** | 2026-11-20 (retrospective; decision taken on 2026-09-28) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (`license = "MIT"` in `[workspace.package]`, "MIT." in the README) |

## 1. Context

The license appears in two places only: `license = "MIT"` in the workspace `Cargo.toml` and one line
in the README. There is no `LICENSE` file. It also constrains dependencies: the Ghostscript engine
(AGPL, shelled out to rather than linked) was one of the reasons for its removal (D-004), and MuPDF,
an obvious PDF library, is AGPL too (D-003).

## 2. Options considered

**A** — MIT.

**B** — `MIT OR Apache-2.0`, the Rust ecosystem's usual dual license (explicit patent grant).

**C** — A copyleft license (GPL/AGPL), which would have allowed linking MuPDF or Ghostscript.

## 3. What the assistant proposed

Not recorded at the time.

## 4. What we decided, and why

**A**, as written in the manifests. Recorded here so that adding a dependency is checked against it:
everything linked so far is permissive (per `cargo metadata`: `mozjpeg-sys` IJG AND Zlib AND
BSD-3-Clause, `jpeg-encoder` (MIT OR Apache-2.0) AND IJG, `lopdf` and `fax` MIT, `zopfli` Apache-2.0,
`hayro-jpeg2000`, `jpeg-decoder`, `image`, `flate2` and Tauri MIT OR Apache-2.0).

## 5. What would make us change our minds

* A contribution or a dependency requiring Apache-2.0's patent grant: switch to B.
* An AGPL library becoming the only way to do something essential (repair, JBIG2): that would be a
  license decision first, not a technical one.

## Open points (review of 2026-11-20)

* Add a `LICENSE` file with the MIT text and the copyright holder: the README line and the manifest
  field alone do not grant anything to someone reading the repository, and the `.dmg`, Windows
  installer and web page ship without a license text or third-party notices (the IJG license of
  mozjpeg and `jpeg-encoder` asks for an acknowledgement in the documentation, and Apache-2.0
  `zopfli` for a copy of its license text).
