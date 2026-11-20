# D-017 — Distribution: a `.dmg` from one script, ad-hoc signed, built by manual workflows

| | |
|---|---|
| **Identifier** | D-017 |
| **Date** | 2026-11-20 (retrospective; decisions taken on 2026-09-28 and 2026-09-30) |
| **Author** | Pascal Havé |
| **Status** | adopted (retrospective) |
| **Commits** | `41f2fdb` (`scripts/build-dmg.sh`, `signingIdentity: "-"`), `7651a19` (`.dmg` built in CI), `9bfb9fb` (moved to manual `macos.yml`), `208345b` (manual `windows.yml`) |

## 1. Context

The app bundles the CLI as a Tauri `externalBin` sidecar (for the Finder service, D-015), and Tauri's
build refuses to run unless that binary is already staged under `app/src-tauri/binaries/`. A `.dmg`
must be reproducible by hand and in CI. Distributing outside the App Store without Gatekeeper warnings
needs a paid Developer ID and notarization.

## 2. Options considered

**A** — Developer ID signing and notarization from the start. No Gatekeeper warning; a paid account,
secrets in CI, and `notarytool` round trips on every build.

**B** — Ad-hoc signing (`signingIdentity: "-"`), documented "right-click › Open" for the first launch,
and the notarization steps written down for later.

For building: **A'** bundle on every push in CI; **B'** check on every push, bundle only on demand.

## 3. What the assistant proposed

**B** with the notarization steps as a comment at the end of `build-dmg.sh`. For CI, **A'** first
(`7651a19`: a `build-app` job uploading the `.dmg`), then **B'** (`9bfb9fb`: "The bundle job moves out
of ci.yml into macos.yml (workflow_dispatch on master only), mirroring windows.yml").

## 4. What we decided, and why

**B** and **B'**:

* **`scripts/build-dmg.sh`** is the one way to build the app: builds the CLI for
  `aarch64-apple-darwin`, stages it as `binaries/pdfshrink-aarch64-apple-darwin`, runs
  `cargo tauri build --bundles dmg`. `macos.yml` just runs it and uploads the result.
* **Ad-hoc signed only**; the README tells users how to get past Gatekeeper. The Developer ID and
  `notarytool`/`stapler` calls are written in the script's trailing comment, ready to fill in.
* **Bundles are built on demand** (`macos.yml`, `windows.yml`, `pages.yml` for the web version, all
  `workflow_dispatch` on master only); pushes and pull requests only run the checks (D-022). There are
  no tags or GitHub releases yet: artefacts live on workflow runs.

Ad-hoc signing was enough for a tool its author and a few people install by hand, and kept secrets
out of CI.

## 5. What would make us change our minds

* Distribution to people who will not right-click › Open, or a Quick Action `.appex` (D-015): both
  need a Developer ID, and then notarization in `macos.yml`.
* A first public version: tags `v*` and a GitHub release carrying the `.dmg`, the Windows installer
  and a link to the web version would replace downloading workflow artefacts (`Alambic` D-034 did
  this).
