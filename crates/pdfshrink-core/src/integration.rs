//! Installs the two OS-level integration points — the Finder service and a
//! `pdfshrink` symlink on `PATH` — shared by the CLI's `install` subcommand and
//! the app's "Install integrations" button so the logic (and the bundled
//! `.workflow` template) lives in exactly one place.
//!
//! The service is an Automator `.workflow` in `~/Library/Services`. As of macOS
//! 26 those land in the contextual menu's *Services* submenu only: the "Quick
//! Actions" submenu (and the Finder list in System Settings → Extensions) is
//! fed exclusively by Action extensions (`.appex`, `NSExtensionPointIdentifier`
//! `com.apple.ui-services`) and Shortcuts, so no `.workflow` can appear there —
//! hence "Finder service", not "Quick Action", in everything the user reads.

use std::path::{Path, PathBuf};
use std::process::Command;

const WORKFLOW_TEMPLATE: &str =
    include_str!("../../../packaging/PdfShrinker.workflow/Contents/document.wflow");
const WORKFLOW_INFO_PLIST: &str =
    include_str!("../../../packaging/PdfShrinker.workflow/Contents/Info.plist");
/// Basename of the CLI binary — the sidecar inside the `.app`, the symlink we
/// drop on `PATH`, and what the service's shell script invokes.
const CLI_NAME: &str = "pdfshrink";

/// The CLI executable the Finder service / CLI symlink should point to.
///
/// Both integrations run `pdfshrink <file>` from a shell, so this has to be the
/// *CLI*, never the GUI binary — pointing the service at `pdfshrinker-app`
/// silently launches the app and the workflow then hangs forever waiting on it.
/// Three cases: inside `PdfShrinker.app` it's the bundled sidecar; when we *are*
/// the CLI it's ourselves; when we're the GUI binary run outside a bundle (i.e.
/// `cargo tauri dev`) it's the `pdfshrink` built next to us.
pub fn resolve_exec_path() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if let Some(app_bundle) = app_bundle_of(&exe) {
        // `externalBin` sidecars are named `<name>-<target-triple>` under
        // `src-tauri/binaries/` but land in the bundle as plain `<name>`.
        return Ok(app_bundle.join("Contents/MacOS").join(CLI_NAME));
    }
    if exe.file_stem().and_then(|s| s.to_str()) == Some(CLI_NAME) {
        return Ok(exe);
    }
    let sibling = exe.with_file_name(CLI_NAME);
    if sibling.exists() {
        return Ok(sibling);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "no `{CLI_NAME}` executable next to {} — build it with `cargo build -p pdfshrink-cli`",
            exe.display()
        ),
    ))
}

fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?; // .../Contents/MacOS
    let contents_dir = macos_dir.parent()?; // .../Contents
    let app_dir = contents_dir.parent()?; // .../PdfShrinker.app
    (app_dir.extension().and_then(|e| e.to_str()) == Some("app")).then(|| app_dir.to_path_buf())
}

/// Install (or update) the "PdfShrinker" Finder service, pointed at `exec`.
pub fn install_finder_service(exec: &Path) -> std::io::Result<PathBuf> {
    let services_dir = home_dir().join("Library/Services");
    std::fs::create_dir_all(&services_dir)?;
    let bundle_dir = services_dir.join("PdfShrinker.workflow");
    let contents_dir = bundle_dir.join("Contents");
    std::fs::create_dir_all(&contents_dir)?;

    let filled = WORKFLOW_TEMPLATE.replace("__PDFSHRINK_EXEC__", &exec.display().to_string());
    std::fs::write(contents_dir.join("document.wflow"), filled)?;
    std::fs::write(contents_dir.join("Info.plist"), WORKFLOW_INFO_PLIST)?;

    // Tell Finder/Services to pick up the new (or updated) service. `-flush`
    // on top of `-update` matters on a *re*install: without it the cached
    // service (with the previous exec path) can stay live until logout.
    for arg in ["-update", "-flush"] {
        let _ = Command::new("/System/Library/CoreServices/pbs")
            .arg(arg)
            .output();
    }

    Ok(bundle_dir)
}

/// Symlink `exec` onto the user's `PATH`.
#[cfg(not(unix))]
pub fn install_cli_symlink(_exec: &Path) -> Result<PathBuf, String> {
    Err("only supported on macOS".into())
}

/// Symlink `exec` onto the user's `PATH`, as `pdfshrink`.
///
/// `/usr/local/bin` is the natural home, but it is `root:wheel` on a stock
/// macOS install, so a GUI app (or any non-`sudo` process) cannot write there.
/// Rather than fail, fall back to the per-user `~/.local/bin`, which always
/// works; the caller tells the user which one it got, and whether that
/// directory is actually on `PATH`.
#[cfg(unix)]
pub fn install_cli_symlink(exec: &Path) -> Result<PathBuf, String> {
    let candidates = [
        PathBuf::from("/usr/local/bin"),
        home_dir().join(".local/bin"),
    ];

    let mut first_error = None;
    for dir in &candidates {
        match try_symlink_into(exec, dir) {
            Ok(link) => return Ok(link),
            Err(e) => first_error.get_or_insert(e),
        };
    }

    let fallback = candidates.last().expect("non-empty candidate list");
    Err(format!(
        "{} (try: sudo ln -sf \"{}\" {}/pdfshrink)",
        first_error.unwrap_or_else(|| "no writable directory on PATH".into()),
        exec.display(),
        fallback.display(),
    ))
}

#[cfg(unix)]
fn try_symlink_into(exec: &Path, dir: &Path) -> Result<PathBuf, String> {
    let link = dir.join(CLI_NAME);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if link.exists() || link.is_symlink() {
        std::fs::remove_file(&link).map_err(|e| e.to_string())?;
    }
    std::os::unix::fs::symlink(exec, &link).map_err(|e| e.to_string())?;
    Ok(link)
}

/// Whether `dir` is one of the entries of the `PATH` this process inherited.
/// A symlink we just dropped into `~/.local/bin` is useless until the user's
/// shell can see it, so the front ends surface this alongside the path.
pub fn is_on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|p| p == dir))
        .unwrap_or(false)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
