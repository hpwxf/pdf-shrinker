//! Installs the two OS-level integration points — the Finder Quick Action and
//! a `pdfshrink` symlink on `PATH` — shared by the CLI's `install` subcommand
//! and the app's "Install integrations" button so the logic (and the bundled
//! `.workflow` template) lives in exactly one place.

use std::path::{Path, PathBuf};
use std::process::Command;

const WORKFLOW_TEMPLATE: &str =
    include_str!("../../../packaging/PdfShrinker.workflow/Contents/document.wflow");
const WORKFLOW_INFO_PLIST: &str =
    include_str!("../../../packaging/PdfShrinker.workflow/Contents/Info.plist");

/// The executable a Quick Action / CLI symlink should point to: when running
/// from inside `PdfShrinker.app`, that's the bundled CLI sidecar (which keeps
/// working across app updates); otherwise it's the current executable itself.
pub fn resolve_exec_path() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if let Some(app_bundle) = app_bundle_of(&exe) {
        // `externalBin` sidecars are named `<name>-<target-triple>` under
        // `src-tauri/binaries/` but land in the bundle as plain `<name>`.
        return Ok(app_bundle.join("Contents/MacOS/pdfshrink"));
    }
    Ok(exe)
}

fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?; // .../Contents/MacOS
    let contents_dir = macos_dir.parent()?; // .../Contents
    let app_dir = contents_dir.parent()?; // .../PdfShrinker.app
    (app_dir.extension().and_then(|e| e.to_str()) == Some("app")).then(|| app_dir.to_path_buf())
}

/// Install (or update) the "PdfShrinker" Finder Quick Action, pointed at `exec`.
pub fn install_quick_action(exec: &Path) -> std::io::Result<PathBuf> {
    let services_dir = home_dir().join("Library/Services");
    std::fs::create_dir_all(&services_dir)?;
    let bundle_dir = services_dir.join("PdfShrinker.workflow");
    let contents_dir = bundle_dir.join("Contents");
    std::fs::create_dir_all(&contents_dir)?;

    let filled = WORKFLOW_TEMPLATE.replace("__PDFSHRINK_EXEC__", &exec.display().to_string());
    std::fs::write(contents_dir.join("document.wflow"), filled)?;
    std::fs::write(contents_dir.join("Info.plist"), WORKFLOW_INFO_PLIST)?;

    // Tell Finder/Services to pick up the new (or updated) service.
    let _ = Command::new("/System/Library/CoreServices/pbs")
        .arg("-update")
        .output();

    Ok(bundle_dir)
}

/// Symlink `exec` as `/usr/local/bin/pdfshrink`.
pub fn install_cli_symlink(exec: &Path) -> Result<PathBuf, String> {
    let link = Path::new("/usr/local/bin/pdfshrink");

    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if link.exists() || link.is_symlink() {
        std::fs::remove_file(link).map_err(|e| e.to_string())?;
    }
    std::os::unix::fs::symlink(exec, link).map_err(|e| {
        format!(
            "{e} (try: sudo ln -sf \"{}\" {})",
            exec.display(),
            link.display()
        )
    })?;

    Ok(link.to_path_buf())
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
