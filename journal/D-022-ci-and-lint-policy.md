# D-022 — CI: macOS arm64, `fmt --check`, `clippy -D warnings`, `test`, all `--locked`; builds on demand

| | |
|---|---|
| **Identifier** | D-022 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-30 and 2026-10-05) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `e94f476` (first `cargo fmt --all`), `7651a19` (`ci.yml`), `9bfb9fb` (bundle moved out), `103a115` (`pages.yml`) |

## 1. Context

Until `7651a19` there was no CI, and `e94f476` found files "never run through rustfmt". The workspace
has a quirk: the Tauri app crate's build refuses to run unless its sidecar binary exists, so
`cargo clippy --workspace` fails on a fresh checkout. The project targets one platform (D-016).

## 2. Options considered

**A** — A matrix (macOS, Linux, Windows) on every push.

**B** — One macOS arm64 job on push and pull requests: `fmt --check`, build and stage the sidecar,
`clippy -D warnings`, `test`, all `--locked`. Bundles and deployments in separate manual workflows.

For the toolchain: pinned in `rust-toolchain.toml`, or `stable`.

## 3. What the assistant proposed

**B**, "macOS-only (macos-14, arm64), matching the project's aarch64-apple-darwin-only scope — no
Linux/Windows matrix" (`7651a19`), with the `.dmg` job split off later (`9bfb9fb`). The toolchain
choice was not recorded.

## 4. What we decided, and why

**B**, `.github/workflows/ci.yml` on `macos-14`:

* `cargo fmt --all -- --check`;
* build the CLI for `aarch64-apple-darwin` and copy it to
  `app/src-tauri/binaries/pdfshrink-aarch64-apple-darwin`, so the whole workspace builds;
* `cargo clippy --workspace --all-targets --locked -- -D warnings`;
* `cargo test --workspace --locked`.

`macos.yml`, `windows.yml` and `pages.yml` are manual (D-017, D-018); `pages.yml` also runs the
pure-Rust core tests on Linux (`--no-default-features`), the only check of the wasm configuration. The
toolchain is `dtolnay/rust-toolchain@stable`, not pinned; edition 2024. Default clippy lints, not
`pedantic`. This is the definition of "done" (D-001).

## 5. What would make us change our minds

* A CI break caused by a new stable release (a new lint under `-D warnings`) rather than by our
  change: then pin the toolchain in `rust-toolchain.toml` and bump it deliberately, as `Alambic` does.
* A wasm-only breakage found at deployment time: the `--no-default-features` core build and tests
  belong in `ci.yml` too.

## Open points (review of 2026-11-20)

* The web build (`wasm32-unknown-unknown`, no `native`) is only checked when someone deploys; a
  `cargo check -p pdfshrink-wasm --target wasm32-unknown-unknown` job on push would catch a
  `native`-only call leaking into shared code (D-018).
* The Windows build is not checked on push either (D-016).
