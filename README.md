# PdfShrinker

Compresses PDFs on macOS — like iLovePDF or UPDF — shrinking file size while staying in PDF format
(no zip). Written in Rust, Apple Silicon only.

Three ways to use it:

- **From the command line** (`pdfshrink`)
- **A macOS app** (`PdfShrinker.app`, installable via `.dmg`), which shows up in Finder › right-click
  a PDF › **Open With**
- **A Finder Quick Action** (right-click a PDF › **Quick Actions** › PdfShrinker), which compresses
  immediately at whatever default level is set in the app

Four compression levels: **Lossless** (structural cleanup only), **Low**, **Medium** (default),
**High**. The compressed file is written next to the original (`name-compressed.pdf`); the original is
never touched, and if the result isn't actually smaller, nothing is written.

## Installation

Download the `.dmg` (see [Build](#build) to produce it yourself), open it, drag `PdfShrinker.app` into
`Applications`. The app isn't signed with an Apple Developer account (ad-hoc signature only), so
Gatekeeper will warn on first launch — right-click the app › **Open**.

On first launch, the **"Install the Quick Action and command-line tool"** button in the app:

- installs the Finder Quick Action (`~/Library/Services`);
- creates the `/usr/local/bin/pdfshrink` symlink pointing at the CLI bundled inside the app.

(Command-line equivalent: `pdfshrink install --quick-action --cli-link`.)

## Usage

### Command line

```bash
pdfshrink file.pdf                          # default level and engine (configurable)
pdfshrink -l high -e best file.pdf           # explicit level and engine
pdfshrink -l medium *.pdf                    # multiple files, in parallel
pdfshrink config get level                   # read a persisted default
pdfshrink config set level low               # change a persisted default
```

Levels (`-l`): `lossless`, `low`, `medium`, `high`.
Engines (`-e`): `rust` (built in, always available), `gs` (Ghostscript, if installed via Homebrew),
`best` (tries both, keeps the smaller one).

Exit codes: `0` success, `1` error, `2` at least one file was already optimal (nothing written for
it).

### App

Drag PDFs into the window (or right-click a PDF › **Open With** › PdfShrinker), pick a level, hit
**Compress**. The **"Set as default level"** checkbox also changes what the Quick Action does, since
both read the same setting.

### Quick Action

Right-click one or more PDFs in Finder › **Quick Actions** › **PdfShrinker**. Compresses immediately
at the app's default level, then shows a macOS notification.

## Build

Requirements: Rust (`rustup target add aarch64-apple-darwin`), Xcode Command Line Tools, `cargo-tauri`
(`cargo install tauri-cli --version "^2"`). Ghostscript is optional, at runtime only
(`brew install ghostscript`) — never bundled (its AGPL license is incompatible with a closed
distribution).

```bash
# CLI only
cargo build --release -p pdfshrink-cli

# App + .dmg (full installer, ad-hoc signed)
./scripts/build-dmg.sh
```

See [`CLAUDE.md`](CLAUDE.md) for the detailed architecture (compression engines, effective-DPI
calculation for images, Tauri app structure, etc.), development commands (`cargo test`,
`cargo tauri dev`, generating a test PDF…) and the current state of verification.

## License

MIT.
