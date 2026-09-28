use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::Document;

use crate::engine::{Engine, Report};
use crate::level::{Level, Profile};
use crate::{PdfShrinkError, Result};

/// Shells out to a locally installed Ghostscript (`gs`), which is not bundled
/// (its license, AGPL, is incompatible with a closed distribution). Only used
/// when the user opts in and `gs` is actually found on the machine.
pub struct GhostscriptEngine;

impl GhostscriptEngine {
    /// Locate the `gs` binary: first on `$PATH`, then in the common Homebrew
    /// locations, since an app launched from Finder doesn't inherit a shell's
    /// PATH and so won't see `/opt/homebrew/bin` even if the user's shell does.
    pub fn find_binary() -> Option<PathBuf> {
        if let Some(path) = find_in_path("gs") {
            return Some(path);
        }
        for candidate in ["/opt/homebrew/bin/gs", "/usr/local/bin/gs", "/usr/bin/gs"] {
            let p = PathBuf::from(candidate);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join(bin);
        candidate.is_file().then_some(candidate)
    })
}

fn pdfsettings_for(level: Level) -> &'static str {
    match level {
        // Lossless is never routed to Ghostscript (see `compress_file`); /printer
        // is the closest, least-lossy preset if this is ever called anyway.
        Level::Lossless | Level::Low => "/printer",
        Level::Medium => "/ebook",
        Level::High => "/screen",
    }
}

impl Engine for GhostscriptEngine {
    fn name(&self) -> &'static str {
        "gs"
    }

    fn is_available(&self) -> bool {
        Self::find_binary().is_some()
    }

    fn compress(&self, input: &Path, output: &Path, profile: &Profile) -> Result<Report> {
        let gs = Self::find_binary().ok_or(PdfShrinkError::GhostscriptNotFound)?;

        let input_size = std::fs::metadata(input)
            .map_err(|e| PdfShrinkError::Io(input.to_path_buf(), e))?
            .len();

        let original_pages = Document::load(input)
            .map_err(|e| PdfShrinkError::InvalidPdf(input.to_path_buf(), e))?
            .get_pages()
            .len();

        let mut cmd = Command::new(&gs);
        cmd.args([
            "-sDEVICE=pdfwrite",
            "-dCompatibilityLevel=1.5",
            "-dNOPAUSE",
            "-dBATCH",
            "-dQUIET",
            "-dSAFER",
        ]);
        cmd.arg(format!("-dPDFSETTINGS={}", pdfsettings_for(profile.level)));
        if profile.level == Level::High {
            cmd.args([
                "-dColorImageResolution=96",
                "-dGrayImageResolution=96",
                "-dMonoImageResolution=200",
            ]);
        }
        cmd.arg(format!("-sOutputFile={}", output.display()));
        cmd.arg(input);

        let result = cmd
            .output()
            .map_err(|e| PdfShrinkError::Io(gs.clone(), e))?;
        if !result.status.success() {
            return Err(PdfShrinkError::GhostscriptFailed(
                input.to_path_buf(),
                String::from_utf8_lossy(&result.stderr).trim().to_string(),
            ));
        }

        let reloaded = Document::load(output)
            .map_err(|e| PdfShrinkError::OutputVerification(output.to_path_buf(), e))?;
        let new_pages = reloaded.get_pages().len();
        if new_pages != original_pages {
            let _ = std::fs::remove_file(output);
            return Err(PdfShrinkError::PageCountMismatch(
                input.to_path_buf(),
                original_pages,
                new_pages,
            ));
        }

        let output_size = std::fs::metadata(output)
            .map_err(|e| PdfShrinkError::Io(output.to_path_buf(), e))?
            .len();

        Ok(Report {
            input_size,
            output_size,
            images_resampled: 0,
            images_skipped: 0,
        })
    }
}
