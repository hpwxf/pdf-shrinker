use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::engine::{Engine, Report};
use crate::error::{PdfShrinkError, Result};
use crate::level::{Level, Profile};
use crate::rust_engine::RustEngine;

/// How hard to compress. `Default` pulls from the persisted [`Config`], which
/// is how the CLI, the GUI and the Quick Action all end up agreeing on "the
/// default level" without talking to each other directly.
#[derive(Debug, Clone, Copy)]
pub struct CompressOptions {
    pub level: Level,
}

impl Default for CompressOptions {
    fn default() -> Self {
        let cfg = Config::load();
        CompressOptions {
            level: cfg.default_level,
        }
    }
}

#[derive(Debug)]
pub enum Outcome {
    Compressed {
        output: PathBuf,
        report: Report,
    },
    /// The engine produced a result, but it wasn't smaller than the original;
    /// nothing was written.
    NotSmaller,
}

/// Compress `input` into a sibling `<name>-compressed.<ext>` file (never
/// overwriting the original, and never overwriting an existing compressed
/// output — `-compressed-2`, `-compressed-3`, … are used instead).
pub fn compress_file(input: &Path, opts: &CompressOptions) -> Result<Outcome> {
    compress_file_with(input, &opts.level.profile(), "compressed")
}

/// Like [`compress_file`], with an explicit (possibly hand-tuned, see
/// [`Profile::tune`](crate::Profile::tune)) profile and output suffix
/// (`<name>-<suffix>.pdf`). Used by the CLI's `--tune`/`--suffix` to try
/// experimental variants side by side.
pub fn compress_file_with(input: &Path, profile: &Profile, suffix: &str) -> Result<Outcome> {
    if !input.is_file() {
        return Err(PdfShrinkError::Io(
            input.to_path_buf(),
            std::io::Error::from(std::io::ErrorKind::NotFound),
        ));
    }

    let output_path = derive_output_path(input, suffix)?;
    let tmp_path = sibling_tmp_path(&output_path, "pdfshrink-tmp");

    let report = RustEngine.compress(input, &tmp_path, profile)?;

    if report.output_size >= report.input_size {
        let _ = fs::remove_file(&tmp_path);
        return Ok(Outcome::NotSmaller);
    }

    fs::rename(&tmp_path, &output_path)
        .map_err(|e| PdfShrinkError::Write(output_path.clone(), e))?;

    Ok(Outcome::Compressed {
        output: output_path,
        report,
    })
}

fn sibling_tmp_path(base: &Path, suffix: &str) -> PathBuf {
    let mut name = base
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output")
        .to_string();
    name.push('.');
    name.push_str(suffix);
    base.with_file_name(name)
}

fn derive_output_path(input: &Path, suffix: &str) -> Result<PathBuf> {
    let stem = input.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
        PdfShrinkError::Io(
            input.to_path_buf(),
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file name"),
        )
    })?;
    let ext = input.extension().and_then(|s| s.to_str()).unwrap_or("pdf");
    let parent = input
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut candidate = parent.join(format!("{stem}-{suffix}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{stem}-{suffix}-{n}.{ext}"));
        n += 1;
    }
    Ok(candidate)
}
